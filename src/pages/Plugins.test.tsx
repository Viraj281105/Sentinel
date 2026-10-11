import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Inspection } from "../bindings/Inspection";
import type { PluginSummary } from "../bindings/PluginSummary";
import { Plugins } from "./Plugins";

// IPC-boundary doubles only.
const SHA = "ab".repeat(32);
const OLD_SHA = "cd".repeat(32);
const FILE = String.raw`C:\Downloads\acme.json`;

const provider = {
  id: "plugin:example.acme:shaders",
  name: "Acme shader cache",
  anchor: "LocalAppData",
  root: String.raw`C:\Users\u\AppData\Local\Acme\Cache`,
  patterns: ["gl", "dx-*"],
  risk: "lowRisk" as const,
  minAgeDays: 5,
  description: "Compiled shaders.",
  onRemoval: "Recompiled on next run.",
};
const summary = (over: Partial<PluginSummary> = {}): PluginSummary => ({
  id: "example.acme",
  name: "Acme caches",
  version: "1.0.0",
  author: "Acme Tools",
  description: "Acme caches.",
  sha256: SHA,
  enabled: true,
  approvedAtMs: Date.UTC(2026, 9, 11),
  problem: null,
  providers: [provider],
  ...over,
});
const inspection = (over: Partial<Inspection> = {}): Inspection => ({
  plugin: summary({ enabled: false, approvedAtMs: 0 }),
  dryRuns: [
    {
      providerId: provider.id,
      root: "scanned",
      eligibleItems: 1,
      eligibleBytes: 5000,
      items: [
        { path: String.raw`C:\Users\u\AppData\Local\Acme\Cache\gl`, bytes: 5000, verdict: "would be moved to quarantine", eligible: true },
      ],
      moreItems: 0,
    },
  ],
  replaces: null,
  alreadyInstalled: false,
  ...over,
});

interface Calls {
  inspected: string[];
  installed: string[];
  toggled: unknown[];
  removed: string[];
}

function backend(opts: { installed?: PluginSummary[]; inspect?: Inspection | "error" } = {}) {
  const calls: Calls = { inspected: [], installed: [], toggled: [], removed: [] };
  let installed = opts.installed ?? [];
  mockIPC((cmd, args) => {
    switch (cmd) {
      case "plugins_list":
        return installed;
      case "plugin:dialog|open":
        return FILE;
      case "plugin_inspect":
        calls.inspected.push((args as { file: string }).file);
        if (opts.inspect === "error") throw { kind: "invalidInput", message: "This is not a usable plugin: risk is wrong." };
        return opts.inspect ?? inspection();
      case "plugin_install": {
        const { sha256 } = args as { sha256: string };
        calls.installed.push(sha256);
        installed = [summary()];
        return installed[0];
      }
      case "plugin_set_enabled":
        calls.toggled.push(args);
        installed = installed.map((p) => ({ ...p, enabled: (args as { enabled: boolean }).enabled }));
        return null;
      case "plugin_remove":
        calls.removed.push((args as { id: string }).id);
        installed = [];
        return null;
      default:
        throw new Error(`unexpected command ${cmd}`);
    }
  });
  return calls;
}

describe("Plugins page", () => {
  it("says plainly that none are installed and nothing needs them", async () => {
    backend();
    render(<Plugins />);
    expect(await screen.findByText(/No plugins are installed/)).toBeInTheDocument();
  });

  it("shows everything needed to decide, and installs nothing until approved", async () => {
    const calls = backend();
    render(<Plugins />);
    await userEvent.click(await screen.findByRole("button", { name: /Choose a plugin file/ }));

    // The review shows who, the exact folder, the patterns, the clamped risk, a live dry run, and the fingerprint.
    const review = await screen.findByText(/This plugin can only ask Sentinel/);
    expect(review).toBeInTheDocument();
    expect(calls.inspected).toEqual([FILE]);
    expect(screen.getByText(SHA)).toBeInTheDocument();
    expect(screen.getByText(String.raw`C:\Users\u\AppData\Local\Acme\Cache`)).toBeInTheDocument();
    expect(screen.getByText("gl, dx-*")).toBeInTheDocument();
    expect(screen.getByText("Low risk")).toBeInTheDocument();
    expect(screen.getAllByText(/4.88 KB/).length).toBeGreaterThanOrEqual(2);
    expect(screen.getByText(/Nothing is moved by approving/)).toBeInTheDocument();
    // Text the plugin wrote is marked as such.
    expect(screen.getAllByText("(written by the plugin)").length).toBeGreaterThanOrEqual(3);
    expect(calls.installed).toHaveLength(0);

    await userEvent.click(screen.getByRole("button", { name: "Approve this plugin" }));
    expect(calls.installed).toEqual([SHA]);
    expect(await screen.findByText("On")).toBeInTheDocument();
    expect(screen.queryByText(/This plugin can only ask Sentinel/)).not.toBeInTheDocument();
  });

  it("cancelling the review installs nothing", async () => {
    const calls = backend();
    render(<Plugins />);
    await userEvent.click(await screen.findByRole("button", { name: /Choose a plugin file/ }));
    await userEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    expect(calls.installed).toHaveLength(0);
    expect(await screen.findByRole("button", { name: /Choose a plugin file/ })).toBeInTheDocument();
  });

  it("shows why a file is refused", async () => {
    backend({ inspect: "error" });
    render(<Plugins />);
    await userEvent.click(await screen.findByRole("button", { name: /Choose a plugin file/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("risk is wrong");
    expect(screen.queryByRole("button", { name: /Approve/ })).not.toBeInTheDocument();
  });

  it("flags a different file under an installed id and cannot re-approve an identical one", async () => {
    backend({
      installed: [summary({ sha256: OLD_SHA })],
      inspect: inspection({ replaces: { version: "1.0.0", sha256: OLD_SHA, enabled: true } }),
    });
    const { unmount } = render(<Plugins />);
    await userEvent.click(await screen.findByRole("button", { name: /Choose a plugin file/ }));
    expect(await screen.findByText(/different file from the version 1.0.0/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Approve and replace" })).toBeEnabled();
    unmount();

    backend({ installed: [summary()], inspect: inspection({ alreadyInstalled: true }) });
    render(<Plugins />);
    await userEvent.click(await screen.findByRole("button", { name: /Choose a plugin file/ }));
    expect(await screen.findByText(/already installed/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Approve this plugin" })).toBeDisabled();
  });

  it("switches a plugin off and on, and removing needs a second step", async () => {
    const calls = backend({ installed: [summary()] });
    render(<Plugins />);
    const card = (await screen.findByText("Acme caches")).closest("section") as HTMLElement;
    await userEvent.click(within(card).getByRole("button", { name: "Switch off" }));
    expect(calls.toggled).toEqual([{ id: "example.acme", enabled: false }]);
    expect(await screen.findByText("Off")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Remove…" }));
    expect(calls.removed).toHaveLength(0);
    expect(screen.getByText(/can still be\s+restored/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(calls.removed).toEqual(["example.acme"]);
    expect(await screen.findByText(/No plugins are installed/)).toBeInTheDocument();
  });

  it("shows a stored plugin that no longer verifies as not in use, with no way to switch it on", async () => {
    backend({
      installed: [summary({ enabled: false, problem: "the stored plugin does not match its recorded fingerprint. It is not used.", providers: [] })],
    });
    render(<Plugins />);
    expect(await screen.findByText("Not in use")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("recorded fingerprint");
    expect(screen.queryByRole("button", { name: /Switch/ })).not.toBeInTheDocument();
  });
});
