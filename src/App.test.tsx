import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import App from "./App";
import type { AppInfo } from "./bindings/AppInfo";
import type { ProtectedLocation } from "./bindings/ProtectedLocation";

// Test doubles for the IPC boundary only; the app itself never ships sample data.
const locations: ProtectedLocation[] = [
  { path: String.raw`C:\Users\u\Documents`, reason: "Documents" },
  { path: String.raw`C:\Windows`, reason: "Windows system files" },
];
const info: AppInfo = { version: "0.1.0", debugBuild: false, logDir: String.raw`C:\logs` };

function mockBackend(fail = false) {
  mockIPC((cmd) => {
    if (fail) throw new Error("backend unavailable");
    if (cmd === "protected_locations") return locations;
    if (cmd === "app_info") return info;
    throw new Error(`unexpected command ${cmd}`);
  });
}

describe("App", () => {
  it("shows the real protected-location count on the overview", async () => {
    mockBackend();
    render(<App />);
    expect(await screen.findByText("2")).toBeInTheDocument();
    expect(screen.getByText(/not implemented yet \(phase 2\)/i)).toBeInTheDocument();
  });

  it("marks unimplemented pages honestly instead of showing data", async () => {
    mockBackend();
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: /storage/i }));
    expect(screen.getByRole("button", { name: /storage/i })).toHaveAttribute("aria-current", "page");
    expect(screen.getByText("Not implemented yet")).toBeInTheDocument();
    expect(screen.getByText(/roadmap phase 2/i)).toBeInTheDocument();
  });

  it("lists protected locations and diagnostics on settings", async () => {
    mockBackend();
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: /settings/i }));
    expect(await screen.findByText(String.raw`C:\Windows`)).toBeInTheDocument();
    expect(screen.getByText("Windows system files")).toBeInTheDocument();
    expect(await screen.findByText(String.raw`C:\logs`)).toBeInTheDocument();
  });

  it("explains backend failures in plain language", async () => {
    mockBackend(true);
    render(<App />);
    expect(await screen.findByRole("alert")).toHaveTextContent("backend unavailable");
  });
});
