import { Cpu, Loader2 } from "lucide-react";
import { useState } from "react";
import type { Runtime } from "../bindings/Runtime";
import type { RuntimeInventory as Inventory } from "../bindings/RuntimeInventory";
import { formatBytes, formatDate } from "../lib/format";
import { describeError, ipc } from "../lib/ipc";
import { RUNTIME_NAMES } from "../lib/runtimes";
import { ErrorNote, Panel } from "./ui";

const leaf = (path: string) => path.split("\\").filter(Boolean).pop() ?? path;

function Results({ inv, at }: { inv: Inventory; at: number }) {
  const groups = (Object.keys(RUNTIME_NAMES) as Runtime[])
    .map((r) => ({ runtime: r, items: inv.installed.filter((i) => i.runtime === r) }))
    .filter((g) => g.items.length > 0);
  return (
    <div className="mt-4 space-y-5 border-t border-slate-200 pt-4 dark:border-slate-800">
      <p className="text-xs text-slate-500 dark:text-slate-400">
        Versions are read from each install's own files or its folder name. Projects are matched only by the version
        they state (<code>engines.node</code>, <code>.python-version</code>, <code>rust-version</code>, ...); a
        requirement Sentinel cannot interpret, such as <code>lts/*</code>, is left out rather than guessed. Compared{" "}
        {inv.projectsRead} project{inv.projectsRead === 1 ? "" : "s"} from your searched folders.
      </p>
      {groups.length === 0 && <p className="text-sm">No runtimes found in the usual install locations.</p>}
      {groups.map((g) => (
        <div key={g.runtime}>
          <h3 className="mb-1 text-sm font-medium">{RUNTIME_NAMES[g.runtime]}</h3>
          <ul className="text-sm">
            {g.items.map((i) => (
              <li key={i.path} className="py-0.5" title={i.path}>
                <span className="flex gap-2">
                  <span className="w-40 shrink-0 truncate font-mono text-xs">{i.version}</span>
                  <span className="w-0 flex-1 truncate text-xs text-slate-500 dark:text-slate-400">
                    {i.label} · {i.installedBy}
                  </span>
                  <span className="w-20 text-right tabular-nums">{i.bytes === null ? "—" : formatBytes(i.bytes)}</span>
                </span>
                {i.usedBy.length > 0 && (
                  <span className="block pl-[10.5rem] text-xs text-emerald-700 dark:text-emerald-400">
                    Meets {i.usedBy.map((u) => `${leaf(u.project)} (${u.requirement})`).join(", ")}
                  </span>
                )}
              </li>
            ))}
          </ul>
        </div>
      ))}
      {inv.unmet.length > 0 && (
        <div>
          <h3 className="mb-1 text-sm font-medium">Requirements no installed version meets</h3>
          <ul className="text-sm">
            {inv.unmet.map((u) => (
              <li key={`${u.project}|${u.runtime}|${u.source}`} className="text-amber-700 dark:text-amber-300">
                {leaf(u.project)} needs {RUNTIME_NAMES[u.runtime]} {u.requirement}{" "}
                <span className="text-xs text-slate-500 dark:text-slate-400">({u.source})</span>
              </li>
            ))}
          </ul>
        </div>
      )}
      {inv.warnings.map((w) => (
        <p key={w} className="text-xs text-amber-700 dark:text-amber-300">
          {w}
        </p>
      ))}
      <p className="text-xs text-slate-500 dark:text-slate-400">
        Inventory only: Sentinel does not uninstall anything. Taken {formatDate(at)}.
      </p>
    </div>
  );
}

/** Read-only inventory of installed development runtimes. */
export function RuntimeInventory() {
  const [state, setState] = useState<
    { s: "idle" } | { s: "loading" } | { s: "done"; inv: Inventory; at: number } | { s: "error"; message: string }
  >({ s: "idle" });
  const run = async () => {
    setState({ s: "loading" });
    try {
      const inv = await ipc.runtimeInventory();
      setState({ s: "done", inv, at: Date.now() });
    } catch (err) {
      setState({ s: "error", message: describeError(err) });
    }
  };
  return (
    <Panel title="Installed runtimes">
      <div className="flex items-start gap-4">
        <p className="flex-1 text-sm text-slate-600 dark:text-slate-300">
          Which versions of Node.js, Python, Java, .NET, Rust, Go and the Android SDK are installed, how much space each
          takes, and which of your projects they satisfy. Nothing is changed.
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
            <Cpu className="size-4" aria-hidden />
          )}
          {state.s === "done" ? "Refresh" : "List runtimes"}
        </button>
      </div>
      {state.s === "error" && (
        <div className="mt-3">
          <ErrorNote message={state.message} />
        </div>
      )}
      {state.s === "done" && <Results inv={state.inv} at={state.at} />}
    </Panel>
  );
}
