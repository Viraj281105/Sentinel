import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { RuntimeInventory as Inventory } from "../bindings/RuntimeInventory";
import { RuntimeInventory } from "./RuntimeInventory";

// IPC-boundary doubles only.
const MB = 1024 ** 2;
const inventory: Inventory = {
  projectsRead: 3,
  warnings: [],
  installed: [
    {
      runtime: "node",
      version: "22.4.0",
      label: "Node.js",
      installedBy: "nvm-windows",
      path: String.raw`C:\Users\u\AppData\Roaming\nvm\v22.4.0`,
      bytes: 100 * MB,
      usedBy: [{ project: String.raw`D:\Projects\web`, requirement: ">=20", source: "package.json engines.node" }],
    },
    {
      runtime: "java",
      version: "21.0.4",
      label: "JDK (Eclipse Adoptium)",
      installedBy: "installer",
      path: String.raw`C:\Program Files\Eclipse Adoptium\jdk-21`,
      bytes: null,
      usedBy: [],
    },
  ],
  unmet: [{ project: String.raw`D:\Projects\old`, runtime: "node", requirement: "16", source: ".nvmrc" }],
};

describe("Installed runtimes", () => {
  it("lists installs on request with what each satisfies", async () => {
    const calls: string[] = [];
    mockIPC((cmd) => {
      calls.push(cmd);
      if (cmd === "runtime_inventory") return inventory;
      throw new Error(`unexpected ${cmd}`);
    });
    render(<RuntimeInventory />);
    expect(calls).toHaveLength(0);
    await userEvent.click(screen.getByRole("button", { name: /List runtimes/ }));

    expect(await screen.findByText("22.4.0")).toBeInTheDocument();
    expect(screen.getByText("Meets web (>=20)")).toBeInTheDocument();
    expect(screen.getByText("JDK (Eclipse Adoptium) · installer")).toBeInTheDocument();
    expect(screen.getByText(/old needs Node\.js 16/)).toBeInTheDocument();
    expect(screen.getByText(/is left out rather than guessed/)).toBeInTheDocument();
    expect(screen.getByText(/Sentinel does not uninstall anything/)).toBeInTheDocument();
  });

  it("says when nothing is installed", async () => {
    mockIPC(() => ({ installed: [], unmet: [], projectsRead: 0, warnings: [] }));
    render(<RuntimeInventory />);
    await userEvent.click(screen.getByRole("button", { name: /List runtimes/ }));
    expect(await screen.findByText(/No runtimes found/)).toBeInTheDocument();
  });
});
