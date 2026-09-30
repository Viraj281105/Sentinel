import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { CleanupRunResponse } from "../bindings/CleanupRunResponse";
import type { Manifest } from "../bindings/Manifest";
import type { ManifestEntry } from "../bindings/ManifestEntry";
import type { Preview } from "../bindings/Preview";
import type { ProviderInfo } from "../bindings/ProviderInfo";
import type { QuarantineContents } from "../bindings/QuarantineContents";
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
  canClean: true,
  note: null,
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

const manifest: Manifest = {
  version: 1,
  operationId: "op-run",
  provider: "user-temp",
  user: "u",
  createdAtMs: Date.UTC(2026, 8, 30, 12),
  expiresAtMs: Date.UTC(2026, 9, 14, 12),
  entries: [
    { index: 0, originalPath: `${T}old-installer`, kind: "folder", bytes: 300 * MB, files: 12, status: { state: "quarantined" } },
    { index: 1, originalPath: `${T}link`, kind: "link", bytes: 0, files: 0, status: { state: "skipped", reason: "it is in use by another program" } },
  ],
};

function mockBackend(opts: { failPreview?: boolean; restoreRefused?: boolean } = {}) {
  const calls: { cmd: string; args: unknown }[] = [];
  let contents: QuarantineContents = {
    root: "C:\\Users\\u\\AppData\\Local\\Sentinel\\.sentinel-quarantine",
    operations: [],
    problems: [],
  };
  mockIPC((cmd, args) => {
    calls.push({ cmd, args });
    switch (cmd) {
      case "cleanup_providers":
        return [provider];
      case "cleanup_preview":
        if (opts.failPreview) throw { kind: "notFound", message: "There is no cleanup type called x." };
        return { preview, operationId: "op-1", auditSeq: 42 };
      case "cleanup_run": {
        contents = { ...contents, operations: [manifest] };
        const res: CleanupRunResponse = { operationId: "op-run", manifest };
        return res;
      }
      case "quarantine_contents":
        return contents;
      case "quarantine_restore": {
        if (opts.restoreRefused) {
          throw { kind: "refused", message: "something already exists there; it will not be overwritten" };
        }
        const [first, second] = manifest.entries as [ManifestEntry, ManifestEntry];
        const restored = { ...first, status: { state: "restored" as const } };
        contents = { ...contents, operations: [{ ...manifest, entries: [restored, second] }] };
        return restored;
      }
      default:
        throw new Error(`unexpected command ${cmd}`);
    }
  });
  return calls;
}

async function previewNow() {
  await userEvent.click(await screen.findByRole("button", { name: /^Preview$/ }));
  await screen.findByText(/Nothing was changed/);
}

async function confirmMove() {
  await userEvent.click(screen.getByRole("button", { name: /Move 2 items to quarantine/ }));
  await userEvent.click(screen.getByRole("button", { name: "Move to quarantine" }));
}

describe("Cleanup page", () => {
  it("says nothing is removed without confirmation and describes each provider", async () => {
    mockBackend();
    render(<Cleanup />);
    expect(screen.getByText("Nothing is removed without your confirmation.")).toBeInTheDocument();
    expect(await screen.findByText("Temporary files for your account")).toBeInTheDocument();
    expect(screen.getByText(/Safe · only items unchanged for 7 days/)).toBeInTheDocument();
    expect(await screen.findByText("Nothing is in quarantine.")).toBeInTheDocument();
  });

  it("previews what would be removed and what would be kept, and why", async () => {
    mockBackend();
    render(<Cleanup />);
    await previewNow();
    expect(screen.getByText(/Recorded in Activity as entry 42/)).toBeInTheDocument();
    expect(screen.getByText(/by moving 2 items \(12 files\) to quarantine/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("tab", { name: /Kept \(2\)/ }));
    expect(screen.getByText("Kept: changed 2 days ago")).toBeInTheDocument();
    expect(screen.getByText("Protected: contains a protected item: .git")).toBeInTheDocument();
  });

  it("asks for confirmation listing exactly the eligible items, and cancel moves nothing", async () => {
    const calls = mockBackend();
    render(<Cleanup />);
    await previewNow();
    await userEvent.click(screen.getByRole("button", { name: /Move 2 items to quarantine/ }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText(/Move 2 items \(300 MB\) to quarantine\?/)).toBeInTheDocument();
    expect(within(dialog).getByText(`${T}old-installer`)).toBeInTheDocument();
    expect(within(dialog).getByText(`${T}link`)).toBeInTheDocument();
    expect(within(dialog).queryByText(`${T}busy.tmp`)).not.toBeInTheDocument();
    expect(within(dialog).getByText(/restore any of them for 14 days/)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(calls.map((c) => c.cmd)).not.toContain("cleanup_run");
  });

  it("moves only the eligible items after confirmation and reports what stayed", async () => {
    const calls = mockBackend();
    render(<Cleanup />);
    await previewNow();
    await confirmMove();
    const status = await screen.findByRole("status");
    expect(status).toHaveTextContent("Moved 1 item (300 MB) to quarantine. 1 left in place.");
    expect(status).toHaveTextContent("Left in place: it is in use by another program");
    expect(calls).toContainEqual({
      cmd: "cleanup_run",
      args: { provider: "user-temp", approved: [`${T}old-installer`, `${T}link`] },
    });
    expect(await screen.findByRole("button", { name: `Restore ${T}old-installer` })).toBeInTheDocument();
    expect(screen.getByText(/then deleted permanently/)).toBeInTheDocument();
  });

  it("restores an item from quarantine", async () => {
    mockBackend();
    render(<Cleanup />);
    await previewNow();
    await confirmMove();
    await userEvent.click(await screen.findByRole("button", { name: `Restore ${T}old-installer` }));
    expect(await screen.findByText("Nothing is in quarantine.")).toBeInTheDocument();
  });

  it("explains a refused restore", async () => {
    mockBackend({ restoreRefused: true });
    render(<Cleanup />);
    await previewNow();
    await confirmMove();
    await userEvent.click(await screen.findByRole("button", { name: `Restore ${T}old-installer` }));
    expect(await screen.findByRole("alert")).toHaveTextContent("it will not be overwritten");
  });

  it("explains a failed preview", async () => {
    mockBackend({ failPreview: true });
    render(<Cleanup />);
    await userEvent.click(await screen.findByRole("button", { name: /^Preview$/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("There is no cleanup type called x.");
  });
});

describe("analysis-only providers", () => {
  it("say why they cannot clean and offer no move", async () => {
    const pnpm: ProviderInfo = {
      ...provider,
      id: "pnpm-store",
      name: "pnpm package store",
      category: "packageCaches",
      minAgeDays: 1,
      canClean: false,
      note: "pnpm's store is hard-linked into every project's node_modules. Run `pnpm store prune`.",
    };
    const skipped: Preview = {
      ...preview,
      provider: pnpm,
      items: [
        {
          path: String.raw`C:\store\v10`,
          kind: "folder",
          bytes: 900 * MB,
          files: 10,
          newestModifiedMs: Date.now() - 40 * DAY,
          decision: { state: "skipped", reason: "analysis only" },
        },
      ],
      eligibleBytes: 0,
      eligibleFiles: 0,
      eligibleItems: 0,
    };
    mockIPC((cmd) => {
      if (cmd === "cleanup_providers") return [pnpm];
      if (cmd === "cleanup_preview") return { preview: skipped, operationId: "op-p", auditSeq: 1 };
      if (cmd === "quarantine_contents") return { root: "q", operations: [], problems: [] };
      throw new Error(`unexpected ${cmd}`);
    });
    render(<Cleanup />);
    expect(await screen.findByText(/Analysis only\./)).toHaveTextContent("pnpm store prune");
    await previewNow();
    expect(screen.queryByRole("button", { name: /to quarantine/ })).not.toBeInTheDocument();
  });
});
