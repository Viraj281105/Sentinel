import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { DependencyGraph } from "../bindings/DependencyGraph";
import { Dependencies } from "./Dependencies";

// IPC-boundary doubles only.
const MB = 1024 ** 2;
const graph: DependencyGraph = {
  nodes: [
    { id: "runtime:a", kind: "runtime", label: "Node.js 22.4.0", detail: String.raw`C:\nvm\v22.4.0`, bytes: 100 * MB },
    { id: "runtime:b", kind: "runtime", label: "Node.js 18.19.1", detail: String.raw`C:\nvm\v18.19.1`, bytes: 90 * MB },
    { id: "runtime:c", kind: "runtime", label: "JDK 21", detail: String.raw`C:\jdk`, bytes: null },
    { id: "project:web", kind: "project", label: "web", detail: String.raw`D:\p\web`, bytes: null },
    { id: "project:old", kind: "project", label: "old", detail: String.raw`D:\p\old`, bytes: null },
    { id: "missing:node:16", kind: "missingRuntime", label: "Node.js 16 (not installed)", detail: "16", bytes: null },
  ],
  edges: [
    { from: "project:web", to: "runtime:a", kind: "requires", note: ">=20 (package.json engines.node)" },
    { from: "project:old", to: "missing:node:16", kind: "unmet", note: "16 (.nvmrc)" },
  ],
  footprints: [
    { runtime: "runtime:a", installBytes: 100 * MB, dependents: ["project:web"], dependentArtifactBytes: 700 * MB },
    { runtime: "runtime:b", installBytes: 90 * MB, dependents: [], dependentArtifactBytes: 0 },
    { runtime: "runtime:c", installBytes: null, dependents: [], dependentArtifactBytes: 0 },
  ],
  unreferencedRuntimes: ["runtime:b"],
  projectsWithoutRequirements: ["project:x", "project:y"],
};

describe("Dependencies page", () => {
  it("builds on request and explains what each link means", async () => {
    const calls: string[] = [];
    mockIPC((cmd) => {
      calls.push(cmd);
      if (cmd === "dependency_graph") return graph;
      throw new Error(`unexpected ${cmd}`);
    });
    render(<Dependencies />);
    expect(calls).toHaveLength(0);
    await userEvent.click(screen.getByRole("button", { name: /Build graph/ }));

    expect(await screen.findByText(">=20 (package.json engines.node)")).toBeInTheDocument();
    expect(screen.getByText(/1 project · 700.* of\s+rebuildable artifacts/)).toBeInTheDocument();
    expect(screen.getByText(/2\s+of your projects state no runtime version/)).toBeInTheDocument();
    expect(screen.getByText(/No stated project requirement needs this version/)).toBeInTheDocument();
    expect(screen.getByText(/does not read project requirements/)).toBeInTheDocument();
    expect(screen.getByText("Requested but not installed")).toBeInTheDocument();
    expect(screen.getByText(/Node\.js 16 \(not installed\)/)).toBeInTheDocument();
  });

  it("says when nothing is installed", async () => {
    mockIPC(() => ({
      nodes: [],
      edges: [],
      footprints: [],
      unreferencedRuntimes: [],
      projectsWithoutRequirements: [],
    }));
    render(<Dependencies />);
    await userEvent.click(screen.getByRole("button", { name: /Build graph/ }));
    expect(await screen.findByText("No installed runtimes were found.")).toBeInTheDocument();
  });
});
