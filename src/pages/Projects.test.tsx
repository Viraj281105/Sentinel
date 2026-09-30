import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Project } from "../bindings/Project";
import type { ProjectSearch } from "../bindings/ProjectSearch";
import { Projects } from "./Projects";

// IPC-boundary doubles only.
const MB = 1024 ** 2;
const DAY = 86_400_000;

function project(name: string, over: Partial<Project> = {}): Project {
  return {
    path: `D:\\Projects\\${name}`,
    name,
    ecosystems: ["node"],
    packageManagers: ["npm"],
    markers: ["package.json"],
    runtimes: [],
    git: true,
    lastActivityMs: Date.now() - DAY,
    totalBytes: 500 * MB,
    artifacts: [],
    warnings: [],
    ...over,
  };
}

const active = project("shop", {
  runtimes: [{ runtime: "node", version: ">=20", source: "package.json engines.node" }],
  artifacts: [
    { kind: "nodeModules", path: "D:\\Projects\\shop\\node_modules", bytes: 300 * MB, restoredBy: "reinstalled by npm" },
  ],
});
const stale = project("old-api", {
  ecosystems: ["python"],
  packageManagers: ["pip"],
  lastActivityMs: Date.now() - 200 * DAY,
  artifacts: [{ kind: "pythonVenv", path: "D:\\Projects\\old-api\\.venv", bytes: 700 * MB, restoredBy: "recreated" }],
  warnings: ["pyproject.toml could not be parsed: bad"],
});

function search(root: string, projects: Project[]): ProjectSearch {
  return {
    root,
    searchedAtMs: Date.now(),
    detection: { root, projects, foldersVisited: 10, truncated: false, cancelled: false, errors: [], elapsedMs: 5 },
  };
}

function mockBackend(initial: ProjectSearch[], picked: string | null = null) {
  const calls: { cmd: string; args: unknown }[] = [];
  mockIPC((cmd, args) => {
    calls.push({ cmd, args });
    switch (cmd) {
      case "project_searches":
        return initial;
      case "plugin:dialog|open":
        return picked;
      case "find_projects":
        return search((args as { root: string }).root, [project("new-one")]);
      case "remove_project_search":
        return true;
      default:
        throw new Error(`unexpected command ${cmd}`);
    }
  });
  return calls;
}

describe("Projects page", () => {
  it("explains what it does before any folder is added", async () => {
    mockBackend([]);
    render(<Projects />);
    expect(await screen.findByText(/never runs install scripts, builds or Git commands/)).toBeInTheDocument();
  });

  it("summarizes rebuildable space and flags inactive projects", async () => {
    mockBackend([search("D:\\Projects", [active, stale])]);
    render(<Projects />);
    expect(await screen.findByText("2 projects")).toBeInTheDocument();
    expect(screen.getByText("1000 MB")).toBeInTheDocument();
    expect(screen.getByText(/belongs to 1 project unchanged for more than 90 days/)).toBeInTheDocument();
    const rows = screen.getAllByRole("button", { expanded: false });
    expect(rows[0]).toHaveTextContent("old-api"); // largest rebuildable first
    expect(rows[0]).toHaveTextContent("inactive");
    expect(rows[1]).not.toHaveTextContent("inactive");
  });

  it("shows package managers, runtimes, artifacts and warnings when expanded", async () => {
    mockBackend([search("D:\\Projects", [active, stale])]);
    render(<Projects />);
    await userEvent.click(await screen.findByRole("button", { name: /shop/ }));
    expect(screen.getByText(/>=20/)).toBeInTheDocument();
    expect(screen.getByText("(package.json engines.node)")).toBeInTheDocument();
    expect(screen.getByText("reinstalled by npm")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /old-api/ }));
    expect(screen.getByText("pyproject.toml could not be parsed: bad")).toBeInTheDocument();
  });

  it("adds a folder picked in the system dialog and searches it", async () => {
    const calls = mockBackend([], "C:\\src");
    render(<Projects />);
    await userEvent.click(await screen.findByRole("button", { name: /Add folder/ }));
    expect(await screen.findByText("new-one")).toBeInTheDocument();
    expect(calls).toContainEqual({ cmd: "find_projects", args: { root: "C:\\src" } });
  });

  it("does nothing when the dialog is cancelled", async () => {
    const calls = mockBackend([], null);
    render(<Projects />);
    await userEvent.click(await screen.findByRole("button", { name: /Add folder/ }));
    expect(calls.map((c) => c.cmd)).not.toContain("find_projects");
  });

  it("stops tracking a folder without touching it", async () => {
    const calls = mockBackend([search("D:\\Projects", [active])]);
    render(<Projects />);
    await userEvent.click(await screen.findByRole("button", { name: "Stop tracking D:\\Projects" }));
    expect(calls).toContainEqual({ cmd: "remove_project_search", args: { root: "D:\\Projects" } });
    expect(screen.queryByText("1 projects")).not.toBeInTheDocument();
    const list = screen.getByText("Folders searched").closest("section") as HTMLElement;
    expect(within(list).queryByText("D:\\Projects")).not.toBeInTheDocument();
  });
});
