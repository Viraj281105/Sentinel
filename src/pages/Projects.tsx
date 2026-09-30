import { open } from "@tauri-apps/plugin-dialog";
import { ChevronDown, ChevronRight, FolderPlus, GitBranch, Loader2, RefreshCw, Trash2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import type { ArtifactKind } from "../bindings/ArtifactKind";
import type { Ecosystem } from "../bindings/Ecosystem";
import type { PackageManager } from "../bindings/PackageManager";
import type { Project } from "../bindings/Project";
import type { ProjectSearch } from "../bindings/ProjectSearch";
import { ErrorNote, Loading, PageHeader, Panel } from "../components/ui";
import { formatAge, formatBytes, formatDateTime } from "../lib/format";
import { describeError, ipc } from "../lib/ipc";

/** A project counts as inactive when nothing in it changed for this long. */
export const INACTIVE_DAYS = 90;
const INACTIVE_MS = INACTIVE_DAYS * 86_400_000;

const ECOSYSTEM: Record<Ecosystem, string> = {
  node: "Node.js",
  python: "Python",
  rust: "Rust",
  java: "Java",
  dotNet: ".NET",
  go: "Go",
  ruby: "Ruby",
  docker: "Docker",
};

const PM: Record<PackageManager, string> = {
  npm: "npm",
  pnpm: "pnpm",
  yarn: "Yarn",
  bun: "Bun",
  pip: "pip",
  poetry: "Poetry",
  pipenv: "Pipenv",
  uv: "uv",
  cargo: "Cargo",
  maven: "Maven",
  gradle: "Gradle",
  dotNet: "dotnet",
  goModules: "Go modules",
  bundler: "Bundler",
};

const ARTIFACT: Record<ArtifactKind, string> = {
  nodeModules: "node_modules",
  pythonVenv: "Virtual environment",
  pythonToolCache: "Python tool cache",
  cargoTarget: "Cargo build output",
  gradleBuild: "Gradle build output",
  gradleProjectCache: "Gradle project cache",
  dotNetBuild: ".NET build output",
  nextBuild: "Next.js build output",
};

const rebuildable = (p: Project) => p.artifacts.reduce((s, a) => s + (a.bytes ?? 0), 0);
const isInactive = (p: Project, now: number) => p.lastActivityMs !== null && now - p.lastActivityMs > INACTIVE_MS;
const leaf = (path: string) => path.split("\\").filter(Boolean).pop() ?? path;

type Sort = "rebuildable" | "activity" | "name";

function Chip({ children }: { children: string }) {
  return (
    <span className="rounded bg-slate-100 px-1.5 py-0.5 text-[11px] text-slate-600 dark:bg-slate-800 dark:text-slate-300">
      {children}
    </span>
  );
}

function ProjectRow({ p, now }: { p: Project; now: number }) {
  const [open, setOpen] = useState(false);
  const inactive = isInactive(p, now);
  const rb = rebuildable(p);
  return (
    <li className="py-2">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className="flex w-full items-start gap-3 rounded-md px-2 py-1 text-left text-sm hover:bg-slate-100 dark:hover:bg-slate-800"
      >
        {open ? (
          <ChevronDown className="mt-0.5 size-4 shrink-0 text-slate-400" aria-hidden />
        ) : (
          <ChevronRight className="mt-0.5 size-4 shrink-0 text-slate-400" aria-hidden />
        )}
        <span className="min-w-0 flex-1">
          <span className="flex flex-wrap items-center gap-1.5">
            <span className="font-medium">{p.name}</span>
            {p.ecosystems.map((e) => (
              <Chip key={e}>{ECOSYSTEM[e]}</Chip>
            ))}
            {p.git && <GitBranch className="size-3.5 text-slate-400" aria-label="Git repository" />}
          </span>
          <span className="block truncate font-mono text-xs text-slate-500 dark:text-slate-400" title={p.path}>
            {p.path}
          </span>
        </span>
        <span className="w-40 shrink-0 text-right text-xs text-slate-500 dark:text-slate-400">
          {p.lastActivityMs === null ? "activity unknown" : `changed ${formatAge(p.lastActivityMs, now)}`}
          {inactive && <span className="ml-1 font-medium text-amber-700 dark:text-amber-300">· inactive</span>}
        </span>
        <span className="w-28 shrink-0 text-right tabular-nums" title="Space in folders the project's tools can rebuild">
          {rb > 0 ? formatBytes(rb) : "—"}
        </span>
        <span
          className="w-24 shrink-0 text-right text-xs tabular-nums text-slate-500 dark:text-slate-400"
          title="Whole project folder"
        >
          {p.totalBytes === null ? "" : formatBytes(p.totalBytes)}
        </span>
      </button>
      {open && (
        <div className="ml-9 mt-2 grid gap-2 text-sm md:grid-cols-2">
          <div>
            <p className="text-xs text-slate-500 dark:text-slate-400">Package managers</p>
            <p>{p.packageManagers.length ? p.packageManagers.map((m) => PM[m]).join(", ") : "None identified"}</p>
            <p className="mt-2 text-xs text-slate-500 dark:text-slate-400">Runtime requirements</p>
            {p.runtimes.length ? (
              <ul>
                {p.runtimes.map((r) => (
                  <li key={`${r.source}-${r.version}`}>
                    {ECOSYSTEM[r.runtime]} {r.version} <span className="text-xs text-slate-500">({r.source})</span>
                  </li>
                ))}
              </ul>
            ) : (
              <p>None declared</p>
            )}
          </div>
          <div>
            <p className="text-xs text-slate-500 dark:text-slate-400">Rebuildable folders</p>
            {p.artifacts.length ? (
              <ul>
                {p.artifacts.map((a) => (
                  <li key={a.path} title={a.path}>
                    {ARTIFACT[a.kind]} <span className="font-mono text-xs">{leaf(a.path)}</span>:{" "}
                    <span className="tabular-nums">{a.bytes === null ? "not measured" : formatBytes(a.bytes)}</span>
                    <span className="block text-xs text-slate-500 dark:text-slate-400">{a.restoredBy}</span>
                  </li>
                ))}
              </ul>
            ) : (
              <p>None found</p>
            )}
            {p.warnings.map((w) => (
              <p key={w} className="mt-1 text-xs text-amber-700 dark:text-amber-300">
                {w}
              </p>
            ))}
          </div>
        </div>
      )}
    </li>
  );
}

export function Projects() {
  const [searches, setSearches] = useState<ProjectSearch[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sort, setSort] = useState<Sort>("rebuildable");
  const [now] = useState(() => Date.now());

  useEffect(() => {
    let cancelled = false;
    ipc.projectSearches().then(
      (s) => !cancelled && setSearches(s),
      (err: unknown) => !cancelled && setError(describeError(err)),
    );
    return () => {
      cancelled = true;
    };
  }, []);

  const run = async (root: string) => {
    setBusy(root);
    setError(null);
    try {
      const s = await ipc.findProjects(root);
      setSearches((prev) => [
        ...(prev ?? []).filter((x) => x.root.toLowerCase() !== s.root.toLowerCase()),
        s,
      ]);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(null);
    }
  };

  const addFolder = async () => {
    const picked = await open({ directory: true, multiple: false, title: "Choose a folder to search for projects" });
    if (typeof picked === "string") await run(picked);
  };

  const remove = async (root: string) => {
    try {
      await ipc.removeProjectSearch(root);
      setSearches((prev) => (prev ?? []).filter((x) => x.root !== root));
    } catch (err) {
      setError(describeError(err));
    }
  };

  const projects = useMemo(() => {
    const seen = new Map<string, Project>();
    for (const s of searches ?? []) for (const p of s.detection.projects) seen.set(p.path.toLowerCase(), p);
    const list = [...seen.values()];
    const by: Record<Sort, (a: Project, b: Project) => number> = {
      rebuildable: (a, b) => rebuildable(b) - rebuildable(a),
      activity: (a, b) => (a.lastActivityMs ?? 0) - (b.lastActivityMs ?? 0),
      name: (a, b) => a.name.localeCompare(b.name),
    };
    return list.sort(by[sort]);
  }, [searches, sort]);

  const totalRebuildable = projects.reduce((s, p) => s + rebuildable(p), 0);
  const inactive = projects.filter((p) => isInactive(p, now));
  const inactiveRebuildable = inactive.reduce((s, p) => s + rebuildable(p), 0);

  return (
    <>
      <PageHeader
        title="Projects"
        subtitle="Development projects in folders you choose, and the space their rebuildable folders use."
      />
      <div className="space-y-4">
        <Panel title="Folders searched">
          {searches === null && !error && <Loading />}
          {searches?.length === 0 && (
            <p className="mb-3 text-sm text-slate-500 dark:text-slate-400">
              Choose a folder where you keep code. Sentinel reads project files there; it never runs install scripts,
              builds or Git commands.
            </p>
          )}
          <ul className="mb-3 divide-y divide-slate-100 text-sm dark:divide-slate-800">
            {searches?.map((s) => (
              <li key={s.root} className="flex items-center gap-3 py-2">
                <span className="flex-1">
                  <span className="font-mono text-xs">{s.root}</span>
                  <span className="block text-xs text-slate-500 dark:text-slate-400">
                    {s.detection.projects.length} projects · searched {formatDateTime(s.searchedAtMs)}
                    {s.detection.truncated && " · stopped at a safety limit, some projects may be missing"}
                  </span>
                </span>
                <button
                  type="button"
                  onClick={() => void run(s.root)}
                  disabled={busy !== null}
                  aria-label={`Search ${s.root} again`}
                  className="rounded-md p-1.5 text-slate-500 hover:bg-slate-100 disabled:opacity-40 dark:hover:bg-slate-800"
                >
                  {busy === s.root ? <Loader2 className="size-4 animate-spin" aria-hidden /> : <RefreshCw className="size-4" aria-hidden />}
                </button>
                <button
                  type="button"
                  onClick={() => void remove(s.root)}
                  disabled={busy !== null}
                  aria-label={`Stop tracking ${s.root}`}
                  className="rounded-md p-1.5 text-slate-500 hover:bg-slate-100 disabled:opacity-40 dark:hover:bg-slate-800"
                >
                  <Trash2 className="size-4" aria-hidden />
                </button>
              </li>
            ))}
          </ul>
          <button
            type="button"
            onClick={() => void addFolder()}
            disabled={busy !== null}
            className="flex items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-emerald-700 disabled:opacity-40"
          >
            {busy !== null && !searches?.some((s) => s.root === busy) ? (
              <Loader2 className="size-4 animate-spin" aria-hidden />
            ) : (
              <FolderPlus className="size-4" aria-hidden />
            )}
            Add folder…
          </button>
          {error && (
            <div className="mt-3">
              <ErrorNote message={error} />
            </div>
          )}
        </Panel>

        {projects.length > 0 && (
          <Panel title={`${projects.length} projects`}>
            <p className="mb-3 text-sm">
              <span className="font-semibold tabular-nums">{formatBytes(totalRebuildable)}</span> in folders the
              projects' own tools can rebuild.{" "}
              {inactive.length > 0 && (
                <>
                  <span className="font-semibold tabular-nums">{formatBytes(inactiveRebuildable)}</span> of that belongs
                  to {inactive.length} {inactive.length === 1 ? "project" : "projects"} unchanged for more than{" "}
                  {INACTIVE_DAYS} days.
                </>
              )}
            </p>
            <label className="mb-2 flex items-center gap-2 text-sm text-slate-500 dark:text-slate-400">
              Sort by
              <select
                value={sort}
                onChange={(e) => setSort(e.target.value as Sort)}
                className="rounded-md border border-slate-300 bg-transparent px-2 py-1 text-slate-900 dark:border-slate-700 dark:text-slate-100"
              >
                <option value="rebuildable">Rebuildable space</option>
                <option value="activity">Least recently changed</option>
                <option value="name">Name</option>
              </select>
            </label>
            <ul className="divide-y divide-slate-100 dark:divide-slate-800">
              {projects.map((p) => (
                <ProjectRow key={p.path} p={p} now={now} />
              ))}
            </ul>
          </Panel>
        )}
      </div>
    </>
  );
}
