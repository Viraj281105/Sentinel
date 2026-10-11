import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { PlanEntry } from "../bindings/PlanEntry";
import type { PlanRunResponse } from "../bindings/PlanRunResponse";
import type { RecoveryPlan } from "../bindings/RecoveryPlan";
import { RecoveryPlanPanel } from "./RecoveryPlanPanel";

// IPC-boundary doubles only.
const MB = 1024 ** 2;
const BIG = String.raw`C:\Users\x\AppData\Local\Temp\big`;
const SMALL = String.raw`C:\Users\x\AppData\Local\Temp\small`;

function entry(path: string, mb: number, running: number): PlanEntry {
  return {
    source: "user-temp",
    sourceName: "Temporary files",
    path,
    kind: "folder",
    bytes: mb * MB,
    risk: "safe",
    onRemoval: "Programs recreate what they need.",
    why: "Chosen first because it is safe.",
    runningBytes: running * MB,
  };
}

const plan: RecoveryPlan = {
  generatedAtMs: 1,
  considered: [],
  plan: {
    targetBytes: 250 * MB,
    maxRisk: "safe",
    entries: [entry(BIG, 200, 200), entry(SMALL, 100, 300)],
    plannedBytes: 300 * MB,
    reachesTarget: true,
    shortfallBytes: 0,
    availableBytes: 300 * MB,
    aboveCeilingBytes: 0,
    aboveCeilingItems: 0,
    notes: [],
  },
};

const ran: PlanRunResponse = {
  parts: [
    {
      source: "user-temp",
      sourceName: "Temporary files",
      error: null,
      run: {
        operationId: "op-1",
        manifest: {
          version: 1,
          operationId: "op-1",
          provider: "user-temp",
          user: "x",
          createdAtMs: 1,
          expiresAtMs: 2,
          entries: [
            { index: 0, originalPath: BIG, kind: "folder", bytes: 200 * MB, files: 3, status: { state: "quarantined" } },
            {
              index: 1,
              originalPath: SMALL,
              kind: "folder",
              bytes: 100 * MB,
              files: 1,
              status: { state: "skipped", reason: "it changed since the preview" },
            },
          ],
        },
      },
    },
  ],
};

async function makePlan() {
  await userEvent.click(screen.getByRole("button", { name: "Make a plan" }));
  return screen.findByRole("button", { name: /Move 2 items to quarantine/ });
}

describe("running a plan", () => {
  it("moves nothing until the full list is confirmed, then sends exactly the plan's items", async () => {
    const calls: unknown[] = [];
    mockIPC((cmd, args) => {
      if (cmd === "plan_recovery") return plan;
      if (cmd === "run_recovery_plan") {
        calls.push(args);
        return ran;
      }
      throw new Error(`unexpected command ${cmd}`);
    });
    let refreshed = 0;
    render(<RecoveryPlanPanel onMoved={() => (refreshed += 1)} />);
    await userEvent.click(await makePlan());

    // The confirmation lists every item, and cancelling moves nothing.
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent(BIG);
    expect(dialog).toHaveTextContent(SMALL);
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(calls).toHaveLength(0);

    await userEvent.click(screen.getByRole("button", { name: /Move 2 items to quarantine/ }));
    await userEvent.click(screen.getByRole("button", { name: "Move to quarantine" }));
    const status = await screen.findByRole("status");
    expect(status).toHaveTextContent("Moved 1 item");
    expect(status).toHaveTextContent("it changed since the preview");
    expect(calls).toEqual([
      {
        approved: [
          { source: "user-temp", path: BIG },
          { source: "user-temp", path: SMALL },
        ],
      },
    ]);
    expect(refreshed).toBe(1);
    // The same, now stale, plan cannot be run twice.
    expect(screen.queryByRole("button", { name: /to quarantine…/ })).not.toBeInTheDocument();
    expect(screen.getByText(/now out of date/)).toBeInTheDocument();
  });

  it("shows a source that could not be run", async () => {
    mockIPC((cmd) => {
      if (cmd === "plan_recovery") return plan;
      return { parts: [{ source: "user-temp", sourceName: "Temporary files", run: null, error: "no such cleanup type" }] };
    });
    render(<RecoveryPlanPanel onMoved={() => {}} />);
    await userEvent.click(await makePlan());
    await userEvent.click(screen.getByRole("button", { name: "Move to quarantine" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Nothing was moved: no such cleanup type");
  });

  it("shows a failed command", async () => {
    mockIPC((cmd) => {
      if (cmd === "plan_recovery") return plan;
      throw { kind: "system", message: "There is no quarantine folder for this account." };
    });
    render(<RecoveryPlanPanel onMoved={() => {}} />);
    await userEvent.click(await makePlan());
    await userEvent.click(screen.getByRole("button", { name: "Move to quarantine" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("no quarantine folder");
  });
});
