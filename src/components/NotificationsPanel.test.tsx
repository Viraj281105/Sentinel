import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Preferences } from "../bindings/Preferences";
import { NotificationsPanel } from "./NotificationsPanel";

// IPC-boundary doubles only.
describe("NotificationsPanel", () => {
  it("is off by default and saves the change at once", async () => {
    const saved: Preferences[] = [];
    mockIPC((cmd, args) => {
      if (cmd === "preferences") return { lowSpaceAlerts: false };
      if (cmd === "set_preferences") {
        const { preferences } = args as { preferences: Preferences };
        saved.push(preferences);
        return preferences;
      }
      throw new Error(`unexpected command ${cmd}`);
    });
    render(<NotificationsPanel />);
    const box = await screen.findByRole("checkbox", { name: /low on space/ });
    expect(box).not.toBeChecked();
    await userEvent.click(box);
    expect(saved).toEqual([{ lowSpaceAlerts: true }]);
    expect(await screen.findByRole("checkbox", { name: /low on space/ })).toBeChecked();
  });

  it("shows a failure to save and leaves the switch as it was", async () => {
    mockIPC((cmd) => {
      if (cmd === "preferences") return { lowSpaceAlerts: false };
      throw { kind: "system", message: "the database failed" };
    });
    render(<NotificationsPanel />);
    await userEvent.click(await screen.findByRole("checkbox", { name: /low on space/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("the database failed");
    expect(screen.getByRole("checkbox", { name: /low on space/ })).not.toBeChecked();
  });
});
