import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { RecoveryPlan } from "../bindings/RecoveryPlan";
import { RecoveryPlanPanel } from "./RecoveryPlanPanel";

// IPC-boundary doubles only.
const MB = 1024 ** 2;
function plan(over: Partial<RecoveryPlan["plan"]> = {}): RecoveryPlan {
  return {
    generatedAtMs: Date.now(),
    projects: [],
    considered: [
      { source: "user-temp", name: "Temporary files", risk: "safe", eligibleItems: 2, eligibleBytes: 300 * MB, note: null },
      {
        source: "project-artifacts",
        name: "Rebuildable folders in inactive projects",
        risk: "mediumRisk",
        eligibleItems: 0,
        eligibleBytes: 0,
        note: "No projects have been searched yet.",
      },
    ],
    plan: {
      targetBytes: 250 * MB,
      maxRisk: "safe",
      entries: [
        {
          source: "user-temp",
          sourceName: "Temporary files",
          path: String.raw`C:\Users\x\AppData\Local\Temp\big`,
          bytes: 200 * MB,
          risk: "safe",
          onRemoval: "Programs recreate what they need.",
          why: "The largest safe item left, so fewer items need to be moved.",
          runningBytes: 200 * MB,
        },
        {
          source: "user-temp",
          sourceName: "Temporary files",
          path: String.raw`C:\Users\x\AppData\Local\Temp\small`,
          bytes: 100 * MB,
          risk: "safe",
          onRemoval: "Programs recreate what they need.",
          why: "The smallest safe item that covers the 50.0 MB still needed.",
          runningBytes: 300 * MB,
        },
      ],
      plannedBytes: 300 * MB,
      reachesTarget: true,
      shortfallBytes: 0,
      availableBytes: 300 * MB,
      aboveCeilingBytes: 0,
      aboveCeilingItems: 0,
      notes: [],
      ...over,
    },
  };
}

describe("RecoveryPlanPanel", () => {
  it("sends the amount in bytes and the chosen ceiling, then explains each item", async () => {
    const calls: unknown[] = [];
    mockIPC((cmd, args) => {
      if (cmd !== "plan_recovery") throw new Error(`unexpected command ${cmd}`);
      calls.push(args);
      return plan();
    });
    render(<RecoveryPlanPanel />);
    const amount = screen.getByLabelText("Amount");
    await userEvent.clear(amount);
    await userEvent.type(amount, "250");
    await userEvent.selectOptions(screen.getByLabelText("Unit"), "MB");
    await userEvent.click(screen.getByRole("button", { name: "Make a plan" }));

    expect(await screen.findByText(/reaching your 250 MB goal/)).toBeInTheDocument();
    expect(calls).toEqual([{ targetBytes: 250 * MB, maxRisk: "safe" }]);
    expect(screen.getByText(/The smallest safe item that covers/)).toBeInTheDocument();
    expect(screen.getByText(/No projects have been searched yet/)).toBeInTheDocument();
  });

  it("offers no ceiling above medium risk", () => {
    mockIPC(() => plan());
    render(<RecoveryPlanPanel />);
    const options = screen.getAllByRole("option").map((o) => o.textContent);
    expect(options).toEqual(["MB", "GB", "Safe", "Low risk", "Medium risk"]);
  });

  it("says plainly when the plan falls short and what exists above the ceiling", async () => {
    mockIPC(() =>
      plan({
        reachesTarget: false,
        shortfallBytes: 700 * MB,
        notes: ["A further 900 MB in 1 items is available at a higher risk. They are not included."],
      }),
    );
    render(<RecoveryPlanPanel />);
    await userEvent.click(screen.getByRole("button", { name: "Make a plan" }));
    expect(await screen.findByText(/700 MB short/)).toBeInTheDocument();
    expect(screen.getByRole("note")).toHaveTextContent("not included");
  });

  it("will not ask for nothing, and shows a refusal", async () => {
    mockIPC(() => {
      throw { kind: "invalidInput", message: "plans can include at most medium-risk items." };
    });
    render(<RecoveryPlanPanel />);
    const amount = screen.getByLabelText("Amount");
    await userEvent.clear(amount);
    await userEvent.type(amount, "0");
    expect(screen.getByRole("button", { name: "Make a plan" })).toBeDisabled();
    await userEvent.clear(amount);
    await userEvent.type(amount, "5");
    await userEvent.click(screen.getByRole("button", { name: "Make a plan" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("at most medium-risk");
  });
});
