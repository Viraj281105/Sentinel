import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { WatchStatus } from "../bindings/WatchStatus";
import { FreshnessBanner } from "./FreshnessBanner";

// IPC-boundary doubles only.
const FINISHED = Date.UTC(2026, 9, 5, 10, 0, 0);
const base: WatchStatus = {
  root: String.raw`D:\Projects`,
  watching: true,
  sinceMs: FINISHED - 1000,
  changedDirs: 0,
  changedSubtrees: 0,
  lostEvents: false,
  broken: null,
  canUpdateIncrementally: true,
  afterRestart: null,
};

function show(status: WatchStatus | "error", onUpdate = vi.fn(), busy = false) {
  mockIPC((cmd) => {
    if (cmd !== "watch_status") throw new Error(`unexpected ${cmd}`);
    if (status === "error") throw new Error("no watcher");
    return status;
  });
  render(<FreshnessBanner scanId={1} finishedAtMs={FINISHED} busy={busy} onUpdate={onUpdate} />);
  return onUpdate;
}

describe("FreshnessBanner", () => {
  it("says nothing changed, without a button", async () => {
    show(base);
    expect(await screen.findByText("No changes seen since this analysis.")).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("admits what it cannot know when watching began after the analysis", async () => {
    show({ ...base, sinceMs: FINISHED + 60_000 });
    expect(await screen.findByText(/Changes before then are not known/)).toBeInTheDocument();
  });

  it("reports what the drive's change record says about the time Sentinel was closed", async () => {
    const view = show({ ...base, sinceMs: FINISHED + 60_000, afterRestart: { state: "verified" } });
    expect(await screen.findByText(/recorded no changes on this drive while Sentinel was closed/)).toBeInTheDocument();
    expect(screen.queryByText(/Changes before then are not known/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(view).not.toHaveBeenCalled();
  });

  it("asks for a full analysis when the drive changed or its record was reset while closed", async () => {
    const changed = show({
      ...base,
      canUpdateIncrementally: false,
      afterRestart: { state: "changedWhileClosed", journalBytes: 4000 },
    });
    expect(await screen.findByText(/changed while Sentinel was closed/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Analyze again" }));
    expect(changed).toHaveBeenCalledTimes(1);
  });

  it("does not claim the drive is unchanged when its record was reset", async () => {
    show({ ...base, canUpdateIncrementally: false, afterRestart: { state: "journalReset" } });
    expect(await screen.findByText(/cannot be known/)).toBeInTheDocument();
    expect(screen.queryByText(/should still be current/)).not.toBeInTheDocument();
  });

  it("offers an incremental update and never starts one by itself", async () => {
    const onUpdate = show({ ...base, changedDirs: 3, changedSubtrees: 1 });
    expect(await screen.findByText("4 folders changed since this analysis.")).toBeInTheDocument();
    expect(onUpdate).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: /re-reads only those folders/ }));
    expect(onUpdate).toHaveBeenCalledTimes(1);
  });

  it("falls back to a full analysis when changes cannot be applied incrementally", async () => {
    show({ ...base, changedDirs: 1, canUpdateIncrementally: false });
    expect(await screen.findByText("1 folder changed since this analysis.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Analyze again" })).toBeInTheDocument();
  });

  it("says plainly when events were lost or the watch broke", async () => {
    show({ ...base, lostEvents: true, broken: "Access is denied.", canUpdateIncrementally: false });
    expect(await screen.findByText(/stopped being able to watch this folder \(Access is denied\.\)/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Analyze again" })).toBeInTheDocument();
  });

  it("disables the button while a scan is running", async () => {
    show({ ...base, changedDirs: 2 }, vi.fn(), true);
    expect(await screen.findByRole("button", { name: /Update/ })).toBeDisabled();
  });

  it("shows nothing when there is no watch or the status cannot be read", async () => {
    show({ ...base, watching: false, root: null });
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("hides itself when the status call fails", async () => {
    show("error");
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });
});
