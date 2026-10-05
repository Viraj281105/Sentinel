import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { InstalledSoftware } from "../bindings/InstalledSoftware";
import { Software } from "./Software";

// IPC-boundary doubles only.
const MB = 1024 ** 2;
function app(name: string, over: Partial<InstalledSoftware> = {}): InstalledSoftware {
  return {
    name,
    version: "1.0",
    publisher: "Acme",
    scope: "machine",
    installDate: null,
    reportedBytes: null,
    installLocation: null,
    msi: false,
    key: name,
    ...over,
  };
}
const inventory = {
  software: [app("Alpha", { reportedBytes: 10 * MB }), app("Beta", { publisher: "Other", reportedBytes: 300 * MB }), app("Gamma")],
  skipped: 7,
  reportedBytes: 310 * MB,
  warnings: [],
};

describe("Software page", () => {
  it("lists programs and is honest about sizes and coverage", async () => {
    mockIPC((cmd) => {
      if (cmd === "software_inventory") return inventory;
      throw new Error(`unexpected ${cmd}`);
    });
    render(<Software />);
    expect(await screen.findByText("Alpha")).toBeInTheDocument();
    expect(screen.getByText("Gamma")).toBeInTheDocument();
    expect(screen.getByText(/many report none/)).toBeInTheDocument();
    expect(screen.getByText(/Winget and Microsoft Store apps are not included yet/)).toBeInTheDocument();
    expect(screen.getByText(/7 updates, system components/)).toBeInTheDocument();
  });

  it("filters by name or publisher and sorts by reported size", async () => {
    mockIPC(() => inventory);
    render(<Software />);
    await screen.findByText("Alpha");

    await userEvent.selectOptions(screen.getByRole("combobox"), "size");
    const names = screen.getAllByRole("listitem").map((li) => li.querySelector("p")?.textContent);
    expect(names).toEqual(["Beta", "Alpha", "Gamma"]);

    await userEvent.type(screen.getByLabelText("Filter software"), "other");
    expect(screen.queryByText("Alpha")).not.toBeInTheDocument();
    expect(screen.getByText("Beta")).toBeInTheDocument();

    await userEvent.clear(screen.getByLabelText("Filter software"));
    await userEvent.type(screen.getByLabelText("Filter software"), "zzz");
    expect(screen.getByText("No programs match.")).toBeInTheDocument();
  });
});
