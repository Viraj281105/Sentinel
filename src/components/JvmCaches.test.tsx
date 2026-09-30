import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { JvmAnalysis } from "../bindings/JvmAnalysis";
import { JvmCaches } from "./JvmCaches";

// IPC-boundary doubles only.
const MB = 1024 ** 2;
const analysis: JvmAnalysis = {
  projectsRead: 1,
  warnings: [],
  maven: {
    root: String.raw`C:\Users\u\.m2\repository`,
    totalBytes: 100 * MB,
    artifactVersions: 452,
    groups: [{ group: "org.springframework", bytes: 10 * MB, versions: 16 }],
    artifacts: [
      {
        group: "io.jsonwebtoken",
        artifact: "jjwt-impl",
        bytes: 2 * MB,
        versions: [
          { version: "0.12.6", bytes: MB, downloadedMs: Date.now(), declaredBy: [String.raw`D:\Projects\app\backend`] },
          { version: "0.11.5", bytes: MB, downloadedMs: Date.now() - 1e10, declaredBy: [] },
        ],
      },
    ],
    artifactsNotListed: 0,
    multiVersionArtifacts: 50,
    olderVersionsBytes: 11 * MB,
    failedDownloads: 0,
    truncated: false,
  },
  gradle: {
    root: String.raw`C:\Users\u\.gradle`,
    totalBytes: 90 * MB,
    modules: null,
    shared: [],
    versions: [
      { version: "8.5", what: "wrapper distribution (bin)", path: "a", bytes: 60 * MB, usedBy: [String.raw`D:\Projects\app`] },
      { version: "7.6", what: "wrapper distribution (all)", path: "b", bytes: 30 * MB, usedBy: [] },
    ],
  },
};

describe("Java build caches", () => {
  it("analyzes on request and states what cannot be known", async () => {
    const calls: string[] = [];
    mockIPC((cmd) => {
      calls.push(cmd);
      if (cmd === "jvm_caches") return analysis;
      throw new Error(`unexpected ${cmd}`);
    });
    render(<JvmCaches />);
    expect(calls).toHaveLength(0);
    await userEvent.click(screen.getByRole("button", { name: /Analyze/ }));

    expect(await screen.findByText(/dates are download dates/)).toBeInTheDocument();
    expect(screen.getByText(/an artifact without it may still be in use/)).toBeInTheDocument();
    expect(screen.getByText(/50 artifacts have more than one version cached/)).toBeInTheDocument();
    expect(screen.getByText("Declared: 0.12.6 by backend")).toBeInTheDocument();
    expect(screen.getByText(/used by app/)).toBeInTheDocument();
    expect(screen.getByText(/no searched project's wrapper uses this version/)).toBeInTheDocument();
    expect(screen.getByText(/Analysis only: Sentinel does not clean these caches/)).toBeInTheDocument();
  });

  it("says when there is nothing to analyze", async () => {
    mockIPC(() => ({ maven: null, gradle: null, projectsRead: 0, warnings: [] }));
    render(<JvmCaches />);
    await userEvent.click(screen.getByRole("button", { name: /Analyze/ }));
    expect(await screen.findByText("No Maven local repository.")).toBeInTheDocument();
    expect(screen.getByText("No Gradle home.")).toBeInTheDocument();
  });
});
