import { useEffect, useMemo, useState } from "react";
import type { InstalledSoftware } from "../bindings/InstalledSoftware";
import type { Scope } from "../bindings/Scope";
import type { SoftwareInventory } from "../bindings/SoftwareInventory";
import { ErrorNote, Loading, PageHeader, Panel } from "../components/ui";
import { formatBytes } from "../lib/format";
import { describeError, ipc } from "../lib/ipc";

type Sort = "name" | "size" | "date";

const SCOPE: Record<Scope, string> = {
  machine: "All users",
  machine32: "All users (32-bit)",
  user: "Current user",
};

const SORTS: Record<Sort, (a: InstalledSoftware, b: InstalledSoftware) => number> = {
  name: () => 0, // the backend order is alphabetical and the sort is stable
  size: (a, b) => (b.reportedBytes ?? -1) - (a.reportedBytes ?? -1),
  date: (a, b) => (b.installDate ?? "").localeCompare(a.installDate ?? ""),
};

function Row({ s }: { s: InstalledSoftware }) {
  return (
    <li className="flex gap-3 py-2 text-sm" title={s.installLocation ?? undefined}>
      <div className="w-0 flex-1">
        <p className="truncate font-medium">{s.name}</p>
        <p className="truncate text-xs text-slate-500 dark:text-slate-400">
          {[s.publisher, s.version, SCOPE[s.scope], s.msi ? "Windows Installer" : null].filter(Boolean).join(" · ")}
        </p>
      </div>
      <span className="w-24 shrink-0 text-xs text-slate-500 dark:text-slate-400">{s.installDate ?? ""}</span>
      <span className="w-24 shrink-0 text-right tabular-nums">
        {s.reportedBytes === null ? "—" : formatBytes(s.reportedBytes)}
      </span>
    </li>
  );
}

export function Software() {
  const [state, setState] = useState<
    { s: "loading" } | { s: "done"; inv: SoftwareInventory } | { s: "error"; message: string }
  >({ s: "loading" });
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<Sort>("name");

  useEffect(() => {
    let live = true;
    ipc
      .softwareInventory()
      .then((inv) => live && setState({ s: "done", inv }))
      .catch((err) => live && setState({ s: "error", message: describeError(err) }));
    return () => {
      live = false;
    };
  }, []);

  const shown = useMemo(() => {
    if (state.s !== "done") return [];
    const q = query.trim().toLowerCase();
    return state.inv.software
      .filter((s) => !q || `${s.name} ${s.publisher ?? ""}`.toLowerCase().includes(q))
      .sort(SORTS[sort]);
  }, [state, query, sort]);

  return (
    <>
      <PageHeader
        title="Software"
        subtitle="Installed applications from the Windows uninstall registry. Read-only: Sentinel does not uninstall anything."
      />
      {state.s === "loading" && <Loading />}
      {state.s === "error" && <ErrorNote message={state.message} />}
      {state.s === "done" && (
        <Panel title={`${state.inv.software.length} programs`}>
          <p className="mb-3 text-xs text-slate-500 dark:text-slate-400">
            Sizes are what each installer reported, and many report none, so the{" "}
            {formatBytes(state.inv.reportedBytes)} total understates real use. Winget and Microsoft Store apps are not
            included yet. {state.inv.skipped} updates, system components and nameless entries were left out.
          </p>
          <div className="mb-2 flex gap-3">
            <input
              type="search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Filter by name or publisher"
              aria-label="Filter software"
              className="w-72 rounded-md border border-slate-300 bg-transparent px-2 py-1 text-sm dark:border-slate-700"
            />
            <label className="flex items-center gap-2 text-sm">
              Sort by
              <select
                value={sort}
                onChange={(e) => setSort(e.target.value as Sort)}
                className="rounded-md border border-slate-300 bg-transparent px-2 py-1 dark:border-slate-700"
              >
                <option value="name">Name</option>
                <option value="size">Reported size</option>
                <option value="date">Install date</option>
              </select>
            </label>
          </div>
          {shown.length === 0 ? (
            <p className="py-3 text-sm">No programs match.</p>
          ) : (
            <ul className="divide-y divide-slate-100 dark:divide-slate-800">
              {shown.map((s) => (
                <Row key={`${s.scope}|${s.key}`} s={s} />
              ))}
            </ul>
          )}
          {state.inv.warnings.map((w) => (
            <p key={w} className="mt-2 text-xs text-amber-700 dark:text-amber-300">
              {w}
            </p>
          ))}
        </Panel>
      )}
    </>
  );
}
