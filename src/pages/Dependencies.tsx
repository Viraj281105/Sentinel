import { Layers, Loader2 } from "lucide-react";
import { useState } from "react";
import type { DependencyGraph } from "../bindings/DependencyGraph";
import { ErrorNote, PageHeader, Panel } from "../components/ui";
import { formatBytes } from "../lib/format";
import { describeError, ipc } from "../lib/ipc";

function Results({ g }: { g: DependencyGraph }) {
  const node = new Map(g.nodes.map((n) => [n.id, n]));
  const unreferenced = new Set(g.unreferencedRuntimes);
  const requiredBy = (runtimeId: string) =>
    g.edges.filter((e) => e.kind === "requires" && e.to === runtimeId).map((e) => ({ e, project: node.get(e.from) }));
  const missing = g.nodes.filter((n) => n.kind === "missingRuntime");
  const installs = g.footprints.filter((f) => node.has(f.runtime));

  return (
    <div className="space-y-6">
      <p className="text-xs text-slate-500 dark:text-slate-400">
        A project is linked to an install only when a version it states (<code>engines.node</code>,{" "}
        <code>.python-version</code>, <code>rust-version</code>, ...) is met by that install. {g.projectsWithoutRequirements.length}{" "}
        of your projects state no runtime version, so Sentinel does not guess which install they use. Artifact sizes are
        rebuildable folders such as <code>node_modules</code>; a project met by several installs is counted under each.
      </p>

      {installs.length === 0 ? (
        <p className="text-sm">No installed runtimes were found.</p>
      ) : (
        installs.map((f) => {
          const n = node.get(f.runtime);
          if (!n) return null;
          const users = requiredBy(f.runtime);
          return (
            <Panel key={f.runtime} title={n.label}>
              <p className="text-xs text-slate-500 dark:text-slate-400" title={n.detail ?? undefined}>
                {n.detail} · install {n.bytes === null ? "size unknown" : formatBytes(n.bytes)}
              </p>
              {users.length > 0 ? (
                <>
                  <ul className="mt-2 text-sm">
                    {users.map(({ e, project }) => (
                      <li key={e.from} className="flex gap-2">
                        <span className="w-0 flex-1 truncate" title={project?.detail ?? undefined}>
                          {project?.label ?? e.from}
                        </span>
                        <span className="text-xs text-slate-500 dark:text-slate-400">{e.note}</span>
                      </li>
                    ))}
                  </ul>
                  <p className="mt-2 text-sm">
                    {users.length} project{users.length === 1 ? "" : "s"} · {formatBytes(f.dependentArtifactBytes)} of
                    rebuildable artifacts
                  </p>
                </>
              ) : unreferenced.has(f.runtime) ? (
                <p className="mt-2 text-sm text-amber-700 dark:text-amber-300">
                  No stated project requirement needs this version. Projects that state none may still use it.
                </p>
              ) : (
                <p className="mt-2 text-sm text-slate-500 dark:text-slate-400">
                  Sentinel does not read project requirements for this kind of install.
                </p>
              )}
            </Panel>
          );
        })
      )}

      {missing.length > 0 && (
        <Panel title="Requested but not installed">
          <ul className="text-sm">
            {missing.map((m) => (
              <li key={m.id}>
                {m.label}
                <span className="text-xs text-slate-500 dark:text-slate-400">
                  {" "}
                  — needed by{" "}
                  {g.edges
                    .filter((e) => e.kind === "unmet" && e.to === m.id)
                    .map((e) => node.get(e.from)?.label ?? e.from)
                    .join(", ")}
                </span>
              </li>
            ))}
          </ul>
        </Panel>
      )}
      <p className="text-xs text-slate-500 dark:text-slate-400">Analysis only: nothing is changed.</p>
    </div>
  );
}

export function Dependencies() {
  const [state, setState] = useState<
    { s: "idle" } | { s: "loading" } | { s: "done"; g: DependencyGraph } | { s: "error"; message: string }
  >({ s: "idle" });
  const run = async () => {
    setState({ s: "loading" });
    try {
      setState({ s: "done", g: await ipc.dependencyGraph() });
    } catch (err) {
      setState({ s: "error", message: describeError(err) });
    }
  };
  return (
    <>
      <PageHeader
        title="Dependencies"
        subtitle="Which of your projects need which installed runtimes, and the disk they account for."
      />
      <div className="mb-6 flex items-start gap-4">
        <p className="flex-1 text-sm text-slate-600 dark:text-slate-300">
          Uses the folders you searched on the Projects page and the runtimes installed on this computer. Measuring
          install sizes can take a few seconds.
        </p>
        <button
          type="button"
          onClick={() => void run()}
          disabled={state.s === "loading"}
          className="flex shrink-0 items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-emerald-700 disabled:opacity-40"
        >
          {state.s === "loading" ? (
            <Loader2 className="size-4 animate-spin" aria-hidden />
          ) : (
            <Layers className="size-4" aria-hidden />
          )}
          {state.s === "done" ? "Refresh" : "Build graph"}
        </button>
      </div>
      {state.s === "error" && <ErrorNote message={state.message} />}
      {state.s === "done" && <Results g={state.g} />}
    </>
  );
}
