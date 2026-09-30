import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { AuditEntry } from "../bindings/AuditEntry";
import type { AuditIntegrity } from "../bindings/AuditIntegrity";
import { Activity } from "./Activity";

// IPC-boundary doubles only.
function entry(seq: number): AuditEntry {
  return {
    seq,
    atMs: Date.UTC(2026, 8, 30, 12, 0),
    operationId: `op-${seq}0000000-aaaa`,
    kind: "preview",
    provider: "user-temp",
    user: "VIRAJ",
    items: seq,
    bytes: 1024 * 1024 * seq,
    policy: "Dry run of 'Temporary files for your account': only items unchanged for 7 days",
    approval: "notRequired",
    outcome: "noChanges",
    errors: seq === 2 ? ["C:\\Temp: access denied"] : [],
    details: {},
    hash: "ab".repeat(32),
  };
}

function mockBackend(integrity: AuditIntegrity, total = 3) {
  const calls: unknown[] = [];
  mockIPC((cmd, args) => {
    if (cmd === "cleanup_providers")
      return [{ id: "user-temp", name: "Temporary files for your account" }];
    if (cmd === "audit_verify") return integrity;
    if (cmd === "audit_log") {
      calls.push(args);
      const { limit, before } = args as { limit: number; before: number | null };
      const start = before === null ? total : before - 1;
      const out: AuditEntry[] = [];
      for (let s = start; s >= 1 && out.length < limit; s--) out.push(entry(s));
      return out;
    }
    throw new Error(`unexpected command ${cmd}`);
  });
  return calls;
}

const intact: AuditIntegrity = { records: 3, intact: true, firstBadSeq: null, problem: null };

describe("Activity page", () => {
  it("shows verified history, newest first, with who, what and why", async () => {
    mockBackend(intact);
    render(<Activity />);
    expect(await screen.findByText(/all 3 entries are intact/)).toBeInTheDocument();
    const items = await screen.findAllByRole("listitem");
    expect(items[0]).toHaveTextContent("entry 3");
    expect(items[0]).toHaveTextContent("Dry run");
    expect(items[0]).toHaveTextContent("Nothing changed");
    expect(items[0]).toHaveTextContent("3 items (3.00 MB) would be moved to quarantine");
    expect(items[0]).toHaveTextContent("VIRAJ");
    expect(await screen.findAllByText(/Temporary files for your account/)).not.toHaveLength(0);
    expect(screen.getByText("C:\\Temp: access denied")).toBeInTheDocument();
  });

  it("warns loudly when the chain is broken", async () => {
    mockBackend({ records: 3, intact: false, firstBadSeq: 2, problem: "contents were changed after it was written" });
    render(<Activity />);
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "History has been altered: entry 2 contents were changed after it was written",
    );
  });

  it("pages backwards through older entries", async () => {
    const calls = mockBackend(intact, 60);
    render(<Activity />);
    await userEvent.click(await screen.findByRole("button", { name: "Load older entries" }));
    expect(await screen.findByText(/entry 1 ·/)).toBeInTheDocument();
    expect(calls).toEqual([
      { limit: 50, before: null },
      { limit: 50, before: 11 },
    ]);
    expect(screen.queryByRole("button", { name: "Load older entries" })).not.toBeInTheDocument();
  });

  it("explains an empty history", async () => {
    mockBackend({ records: 0, intact: true, firstBadSeq: null, problem: null }, 0);
    render(<Activity />);
    expect(await screen.findByText(/Nothing recorded yet/)).toBeInTheDocument();
  });
});
