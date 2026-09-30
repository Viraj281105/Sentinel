import { Eye, Loader2 } from "lucide-react";
import { useState } from "react";
import type { PreviewResponse } from "../bindings/PreviewResponse";
import { formatBytes } from "../lib/format";
import { describeError, ipc } from "../lib/ipc";
import { PreviewResult } from "./PreviewResult";
import { ErrorNote, Panel } from "./ui";

interface Props {
  /** Paths of the projects that looked inactive in the saved search results. */
  projects: string[];
  /** Their rebuildable space according to the saved results. */
  bytes: number;
}

/** Preview and clean up the rebuildable folders of inactive projects. */
export function ProjectCleanup({ projects, bytes }: Props) {
  const [state, setState] = useState<
    { s: "idle" } | { s: "loading" } | { s: "done"; response: PreviewResponse } | { s: "error"; message: string }
  >({ s: "idle" });

  const run = async () => {
    setState({ s: "loading" });
    try {
      setState({ s: "done", response: await ipc.projectCleanupPreview(projects) });
    } catch (err) {
      setState({ s: "error", message: describeError(err) });
    }
  };

  return (
    <Panel title="Clean up inactive projects">
      <div className="flex items-start gap-4">
        <div className="flex-1 space-y-1.5 text-sm">
          <p>
            {projects.length} {projects.length === 1 ? "project has" : "projects have"} not changed for 90 days and
            hold <span className="font-semibold tabular-nums">{formatBytes(bytes)}</span> in rebuildable folders
            (node_modules, virtual environments, build output).
          </p>
          <p className="text-slate-500 dark:text-slate-400">
            A preview checks each project again first. Folders Git tracks, projects changed recently and anything
            uncertain are left alone. Moved folders stay restorable for 14 days; the project needs a reinstall or
            rebuild before it runs again.
          </p>
        </div>
        <button
          type="button"
          onClick={() => void run()}
          disabled={state.s === "loading"}
          className="flex shrink-0 items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-emerald-700 disabled:opacity-40"
        >
          {state.s === "loading" ? (
            <Loader2 className="size-4 animate-spin" aria-hidden />
          ) : (
            <Eye className="size-4" aria-hidden />
          )}
          {state.s === "done" ? "Preview again" : "Preview cleanup"}
        </button>
      </div>
      {state.s === "error" && (
        <div className="mt-4">
          <ErrorNote message={state.message} />
        </div>
      )}
      {state.s === "done" && (
        <PreviewResult
          key={state.response.operationId}
          response={state.response}
          providerName="Rebuildable folders in inactive projects"
          onRun={(paths) => ipc.projectCleanupRun(projects, paths)}
          onMoved={() => undefined}
        />
      )}
    </Panel>
  );
}
