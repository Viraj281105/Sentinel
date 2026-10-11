import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { AiSettings } from "../bindings/AiSettings";
import type { AskResponse } from "../bindings/AskResponse";
import { Assistant } from "./Assistant";
import { Settings } from "./Settings";

// IPC-boundary doubles only.
const off: AiSettings = { enabled: false, baseUrl: "http://127.0.0.1:11434", model: "llama3.2" };
const on: AiSettings = { ...off, enabled: true };

const answer: AskResponse = {
  aiGenerated: true,
  provider: "ollama",
  model: "llama3.2",
  sentQuestion: "old folders in <dir-1>",
  queryDescription: "Folders, not changed for 90 days or more, largest first, up to 20",
  result: {
    scanId: 1,
    root: "D:\\",
    finishedAtMs: Date.UTC(2026, 9, 10),
    target: "folders",
    description: "Folders",
    matches: 1,
    items: [
      {
        path: String.raw`D:\Projects\old\node_modules`,
        bytes: 3 * 1024 * 1024,
        files: 10,
        lastChangedMs: Date.UTC(2026, 3, 1),
        classification: { category: "developerDependencies", rule: "node-modules", reason: "node_modules folder" },
      },
    ],
    uniqueBytes: 3 * 1024 * 1024,
    moreThanShown: false,
    notApplied: [{ field: "olderThanDays", reason: "This analysis has no record of when folders last changed." }],
    notes: ["Folders smaller than 1048576 bytes were not stored in this analysis, so they cannot match."],
  },
};

describe("Assistant page", () => {
  it("is off by default and offers no question box", async () => {
    mockIPC((cmd) => {
      if (cmd === "ai_settings") return off;
      throw new Error(`unexpected command ${cmd}`);
    });
    render(<Assistant />);
    expect(await screen.findByText(/The AI assistant is off/)).toBeInTheDocument();
    expect(screen.queryByLabelText("Your question")).not.toBeInTheDocument();
  });

  it("labels the answer as AI-made and shows what was sent, the search, and the results", async () => {
    const asked: unknown[] = [];
    mockIPC((cmd, args) => {
      if (cmd === "ai_settings") return on;
      if (cmd === "ask_assistant") {
        asked.push(args);
        return answer;
      }
      throw new Error(`unexpected command ${cmd}`);
    });
    render(<Assistant />);
    await userEvent.type(await screen.findByLabelText("Your question"), "old folders in C:\\Clients\\Acme");
    await userEvent.click(screen.getByRole("button", { name: "Ask" }));

    expect(await screen.findByText(/AI answered this · ollama · llama3.2/)).toBeInTheDocument();
    expect(screen.getByText("old folders in <dir-1>")).toBeInTheDocument();
    expect(screen.getByText(/Folders, not changed for 90 days or more/)).toBeInTheDocument();
    expect(screen.getByText(String.raw`D:\Projects\old\node_modules`)).toBeInTheDocument();
    // What the data could not answer is stated, not hidden.
    expect(screen.getByRole("note")).toHaveTextContent("Not applied");
    expect(asked).toHaveLength(1);
  });

  it("shows a refusal or failure as an error and no results", async () => {
    mockIPC((cmd) => {
      if (cmd === "ai_settings") return on;
      if (cmd === "ask_assistant")
        throw { kind: "system", message: "the local model could not be reached. Is Ollama running?" };
      throw new Error(`unexpected command ${cmd}`);
    });
    render(<Assistant />);
    await userEvent.type(await screen.findByLabelText("Your question"), "big files");
    await userEvent.click(screen.getByRole("button", { name: "Ask" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Is Ollama running?");
    expect(screen.queryByText(/AI answered this/)).not.toBeInTheDocument();
  });
});

describe("AI settings in Settings", () => {
  it("saves a change and shows a refusal for a non-local address", async () => {
    const saved: unknown[] = [];
    mockIPC((cmd, args) => {
      if (cmd === "ai_settings") return off;
      if (cmd === "app_info") return { version: "0.1.0", debugBuild: true, logDir: "L", databasePath: "D", databaseError: null };
      if (cmd === "protected_locations") return [];
      if (cmd === "set_ai_settings") {
        const { settings } = args as { settings: AiSettings };
        if (!settings.baseUrl.startsWith("http://127.")) throw { kind: "refused", message: "not on this computer" };
        saved.push(settings);
        return settings;
      }
      throw new Error(`unexpected command ${cmd}`);
    });
    render(<Settings />);
    const save = await screen.findByRole("button", { name: "Save" });
    expect(save).toBeDisabled();
    await userEvent.click(screen.getByLabelText("Turn on the AI assistant"));
    await userEvent.click(save);
    expect(saved).toEqual([on]);

    const url = screen.getByLabelText("Ollama address");
    await userEvent.clear(url);
    await userEvent.type(url, "https://api.example.com");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("not on this computer");
    expect(saved).toHaveLength(1);
  });
});
