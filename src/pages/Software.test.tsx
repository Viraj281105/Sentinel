import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen, within } from "@testing-library/react";
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
  storeApps: [
    { name: "Calculator", identity: "Microsoft.WindowsCalculator", version: "11.0", architecture: "x64", kind: "app", rootFolder: null },
    { name: "Microsoft.VCLibs.140.00", identity: "Microsoft.VCLibs.140.00", version: "14.0", architecture: "x64", kind: "framework", rootFolder: null },
  ],
  storeSkipped: 3,
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
    expect(screen.getByText(/Winget is not asked/)).toBeInTheDocument();
    expect(screen.getByText(/7 updates, system components/)).toBeInTheDocument();
  });

  it("shows Store apps and hides frameworks until asked", async () => {
    mockIPC(() => inventory);
    render(<Software />);
    expect(await screen.findByText("Calculator")).toBeInTheDocument();
    expect(screen.queryByText("Microsoft.VCLibs.140.00")).not.toBeInTheDocument();
    expect(screen.getByText(/3 language packs, older versions/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("checkbox", { name: /Include frameworks/ }));
    expect(screen.getByText("Microsoft.VCLibs.140.00")).toBeInTheDocument();
  });

  it("filters by name or publisher and sorts by reported size", async () => {
    mockIPC(() => inventory);
    render(<Software />);
    await screen.findByText("Alpha");

    await userEvent.selectOptions(screen.getByRole("combobox"), "size");
    const [programs] = screen.getAllByRole("list");
    if (!programs) throw new Error("no program list");
    const names = within(programs).getAllByRole("listitem").map((li) => li.querySelector("p")?.textContent);
    expect(names).toEqual(["Beta", "Alpha", "Gamma"]);

    await userEvent.type(screen.getByLabelText("Filter software"), "other");
    expect(screen.queryByText("Alpha")).not.toBeInTheDocument();
    expect(screen.getByText("Beta")).toBeInTheDocument();

    await userEvent.clear(screen.getByLabelText("Filter software"));
    await userEvent.type(screen.getByLabelText("Filter software"), "zzz");
    expect(screen.getByText("No programs match.")).toBeInTheDocument();
  });
});
