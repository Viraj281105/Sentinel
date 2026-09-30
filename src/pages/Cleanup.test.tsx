import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Preview } from "../bindings/Preview";
import type { ProviderInfo } from "../bindings/ProviderInfo";
import { Cleanup } from "./Cleanup";

// IPC-boundary doubles only.
const MB = 1024 ** 2;
const DAY = 86_400_000;
const provider: ProviderInfo = {
  id: "user-temp",
  name: "Temporary files for your account",
  category: "temporaryFiles",
  risk: "safe",
  description: "Files that programs create in your temporary folder.",
  onRemoval: "Programs recreate temporary files when they need them.",
  minAgeDays: 7,
};
const T = "C:\\Users\\u\\AppData\\Local\\Temp\\";
const preview: Preview = {
  provider,
  dryRun: true,
  generatedAtMs: Date.now(),
  roots: [{ state: "scanned", path: "C:\\Users\\u\\AppData\\Local\\Temp" }],
  items: [
    { path: `${T}old-installer`, kind: "folder", bytes: 300 * MB, files: 12, newestModifiedMs: Date.now() - 40 * DAY, decision: { state: "eligible" } },
    { path: `${T}busy.tmp`, kind: "file", bytes: 50 * MB, files: 1, newestModifiedMs: Date.now() - 2 * DAY, decision: { state: "tooRecent", newestModifiedMs: Date.now() - 2 * DAY } },
    { path: `${T}clone`, kind: "folder", bytes: 0, files: 0, newestModifiedMs: null, decision: { state: "protected", reason: "contains a protected item: .git" } },
    { path: `${T}link`, kind: "link", bytes: 0, files: 0, newestModifiedMs: Date.now() - 40 * DAY, decision: { state: "eligible" } },
  ],
  eligibleBytes: 300 * MB,
  eligibleFiles: 12,
  eligibleItems: 2,
  incomplete: false,
};

function mockBackend(fail = false) {
  const calls: string[] = [];
  mockIPC((cmd, args) => {
    calls.push(cmd);
    if (cmd === "cleanup_providers") return [provider];
    if (cmd === "cleanup_preview") {
      if (fail) throw { kind: "notFound", message: "There is no cleanup type called x." };
      expect(args).toEqual({ provider: "user-temp" });
      return preview;
    }
    throw new Error(`unexpected command ${cmd}`);
  });
  return calls;
}

describe("Cleanup page", () => {
  it("states plainly that it is preview-only and describes each provider", async () => {
    mockBackend();
    render(<Cleanup />);
    expect(screen.getByText("Preview only.")).toBeInTheDocument();
    expect(await screen.findByText("Temporary files for your account")).toBeInTheDocument();
    expect(screen.getByText(/Safe · only items unchanged for 7 days/)).toBeInTheDocument();
    expect(screen.getByText("Programs recreate temporary files when they need them.")).toBeInTheDocument();
  });

  it("previews what would be removed and what would be kept, and why", async () => {
    const calls = mockBackend();
    render(<Cleanup />);
    await userEvent.click(await screen.findByRole("button", { name: /^Preview$/ }));
    expect(await screen.findByText(/Nothing was changed/)).toBeInTheDocument();
    expect(screen.getAllByText("300 MB").length).toBeGreaterThan(0);
    expect(screen.getByText(/by moving 2 items \(12 files\) to quarantine/)).toBeInTheDocument();
    expect(screen.getByText("link only")).toBeInTheDocument();
    expect(screen.queryByText(/busy\.tmp/)).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("tab", { name: /Kept \(2\)/ }));
    expect(screen.getByText(/busy\.tmp/)).toBeInTheDocument();
    expect(screen.getByText("Kept: changed 2 days ago")).toBeInTheDocument();
    expect(screen.getByText("Protected: contains a protected item: .git")).toBeInTheDocument();
    expect(calls.filter((c) => c === "cleanup_preview")).toHaveLength(1);
  });

  it("explains a failed preview", async () => {
    mockBackend(true);
    render(<Cleanup />);
    await userEvent.click(await screen.findByRole("button", { name: /^Preview$/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("There is no cleanup type called x.");
  });
});
