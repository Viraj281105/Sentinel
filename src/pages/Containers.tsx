import { Container, Loader2 } from "lucide-react";
import { useState } from "react";
import type { DockerOverview } from "../bindings/DockerOverview";
import type { PruneCategory } from "../bindings/PruneCategory";
import type { PruneRisk } from "../bindings/PruneRisk";
import type { WslInventory } from "../bindings/WslInventory";
import { ErrorNote, PageHeader, Panel } from "../components/ui";
import { formatBytes, formatDate } from "../lib/format";
import { describeError, ipc } from "../lib/ipc";

/** Docker prints decimal units (1 GB = 1000 MB), so its figures are shown the same way. */
function decimal(bytes: number): string {
  const units = ["B", "kB", "MB", "GB", "TB"];
  let v = bytes;
  let u = 0;
  while (v >= 1000 && u < units.length - 1) {
    v /= 1000;
    u += 1;
  }
  return `${u === 0 ? v : v.toFixed(v < 10 ? 2 : v < 100 ? 1 : 0)} ${units[u]}`;
}
const approx = (b: number | null) => (b === null ? "—" : `≈ ${decimal(b)}`);

const RISK: Record<PruneRisk, { label: string; cls: string }> = {
  low: { label: "Low risk", cls: "text-emerald-700 dark:text-emerald-400" },
  medium: { label: "Medium risk", cls: "text-amber-700 dark:text-amber-300" },
  high: { label: "High risk: can hold data that exists nowhere else", cls: "text-red-700 dark:text-red-400" },
};
const SHOWN = 12;

function PruneCard({ c }: { c: PruneCategory }) {
  const risk = RISK[c.risk];
  return (
    <li className="py-3">
      <div className="flex flex-wrap items-baseline gap-x-3">
        <h3 className="text-sm font-medium">{c.title}</h3>
        <span className={`text-xs ${risk.cls}`}>{risk.label}</span>
        <span className="ml-auto text-xs text-slate-500 dark:text-slate-400">
          {c.items.length} item{c.items.length === 1 ? "" : "s"}
          {c.itemBytes !== null && ` · listed sizes add up to ${approx(c.itemBytes)}`}
        </span>
      </div>
      <p className="mt-1 text-sm">
        {c.dockerReclaimableBytes === null
          ? "Docker gives no reclaimable estimate for this."
          : `Docker's own estimate of what this frees: ${approx(c.dockerReclaimableBytes)}.`}
      </p>
      <p className="mt-1 text-xs text-slate-500 dark:text-slate-400">{c.caution}</p>
      {c.items.length > 0 && (
        <details className="mt-1">
          <summary className="cursor-pointer text-xs text-slate-600 dark:text-slate-300">Show what is affected</summary>
          <ul className="mt-1 text-sm">
            {c.items.slice(0, SHOWN).map((i) => (
              <li key={`${i.label}|${i.detail}`} className="flex gap-2 py-0.5">
                <span className="w-0 flex-1 truncate font-mono text-xs" title={i.label}>
                  {i.label}
                </span>
                <span className="w-0 flex-1 truncate text-xs text-slate-500 dark:text-slate-400" title={i.detail}>
                  {i.detail}
                </span>
                <span className="w-24 text-right tabular-nums">{i.bytes === null ? "" : approx(i.bytes)}</span>
              </li>
            ))}
          </ul>
          {c.items.length > SHOWN && (
            <p className="text-xs text-slate-500 dark:text-slate-400">and {c.items.length - SHOWN} more</p>
          )}
        </details>
      )}
      <p className="mt-1 text-xs text-slate-500 dark:text-slate-400">
        Docker's command: <code>{c.command}</code>
      </p>
    </li>
  );
}

function PrunePreview({ d }: { d: DockerOverview }) {
  if (!d.prunePreview) return null;
  return (
    <Panel title="What Docker's cleanup commands would remove (preview)">
      <p className="text-xs text-slate-500 dark:text-slate-400">
        This is only a preview built from what Docker reported. Sentinel does not run these commands and has no way to
        undo them: pruned images, cache and volumes are deleted outright, not quarantined. The categories overlap, so
        they are not added into one total. If you decide to clean up, run Docker's command yourself in a terminal.
      </p>
      <ul className="divide-y divide-slate-100 dark:divide-slate-800">
        {d.prunePreview.categories.map((c) => (
          <PruneCard key={c.id} c={c} />
        ))}
      </ul>
    </Panel>
  );
}

function DockerPanels({ d }: { d: DockerOverview }) {
  if (d.state === "notInstalled") {
    return (
      <Panel title="Docker">
        <p className="text-sm">Docker is not installed (no docker.exe found).</p>
      </Panel>
    );
  }
  if (d.state === "notRunning") {
    return (
      <Panel title="Docker">
        <p className="text-sm">
          Docker is installed but its engine is not running, so nothing can be listed. Sentinel does not start Docker
          for you; start Docker Desktop and refresh.
        </p>
        {d.warnings.map((w) => (
          <p key={w} className="mt-1 text-xs text-amber-700 dark:text-amber-300">
            {w}
          </p>
        ))}
      </Panel>
    );
  }
  return (
    <>
      <Panel title={`Docker ${d.serverVersion ?? ""}`.trim()}>
        <p className="mb-2 text-xs text-slate-500 dark:text-slate-400">
          Sizes are the rounded figures Docker prints, so they are approximate. Sentinel only reads: nothing is pruned,
          stopped or removed, and volumes can hold databases and other data you cannot recreate.
        </p>
        <ul className="text-sm">
          {d.usage.map((u) => (
            <li key={u.kind} className="flex gap-2 py-0.5">
              <span className="w-36">{u.kind}</span>
              <span className="w-28 text-xs text-slate-500 dark:text-slate-400">
                {u.total ?? "?"} total · {u.active ?? "?"} active
              </span>
              <span className="w-28 text-right tabular-nums">{approx(u.sizeBytes)}</span>
              <span className="flex-1 text-right text-xs text-slate-500 dark:text-slate-400">
                {u.reclaimableBytes !== null && u.reclaimableBytes > 0
                  ? `Docker counts ${approx(u.reclaimableBytes)} as reclaimable`
                  : ""}
              </span>
            </li>
          ))}
        </ul>
        {d.warnings.map((w) => (
          <p key={w} className="mt-2 text-xs text-amber-700 dark:text-amber-300">
            {w}
          </p>
        ))}
      </Panel>

      <PrunePreview d={d} />

      <Panel title={`Images (${d.images.length})`}>
        {d.images.length === 0 ? (
          <p className="text-sm">No images.</p>
        ) : (
          <ul className="text-sm">
            {d.images.map((i) => (
              <li key={`${i.id}|${i.repository}|${i.tag}`} className="flex gap-2 py-0.5">
                <span className="w-0 flex-1 truncate font-mono text-xs" title={i.id}>
                  {i.dangling ? "<untagged>" : `${i.repository}:${i.tag}`}
                </span>
                <span className="text-xs text-slate-500 dark:text-slate-400">
                  {i.created} · {i.containers === 0 ? "no containers" : `${i.containers} container${i.containers === 1 ? "" : "s"}`}
                </span>
                <span className="w-24 text-right tabular-nums">{approx(i.sizeBytes)}</span>
              </li>
            ))}
          </ul>
        )}
      </Panel>

      <Panel title={`Containers (${d.containers.length})`}>
        {d.containers.length === 0 ? (
          <p className="text-sm">No containers.</p>
        ) : (
          <ul className="text-sm">
            {d.containers.map((c) => (
              <li key={c.id} className="flex gap-2 py-0.5">
                <span className="w-0 flex-1 truncate">{c.name}</span>
                <span className="w-0 flex-1 truncate font-mono text-xs">{c.image}</span>
                <span className="text-xs text-slate-500 dark:text-slate-400">{c.status}</span>
              </li>
            ))}
          </ul>
        )}
      </Panel>

      <Panel title={`Volumes (${d.volumes.length})`}>
        {d.volumes.length === 0 ? (
          <p className="text-sm">No volumes.</p>
        ) : (
          <ul className="text-sm">
            {d.volumes.map((v) => (
              <li key={v.name} className="flex gap-2 py-0.5">
                <span className="w-0 flex-1 truncate font-mono text-xs" title={v.name}>
                  {v.name}
                </span>
                <span className="text-xs text-slate-500 dark:text-slate-400">
                  {v.composeProject ? `Compose: ${v.composeProject} · ` : ""}
                  {v.links === null ? "" : v.links === 0 ? "no container uses it" : `${v.links} in use`}
                </span>
                <span className="w-24 text-right tabular-nums">{approx(v.sizeBytes)}</span>
              </li>
            ))}
          </ul>
        )}
      </Panel>
    </>
  );
}

function WslPanel({ w }: { w: WslInventory }) {
  return (
    <Panel title={`WSL distributions (${w.distros.length})`}>
      <p className="mb-2 text-xs text-slate-500 dark:text-slate-400">
        Read from the registry; Sentinel does not run wsl.exe, so running state is not shown. Each distribution's disk is
        its whole filesystem and only grows until it is compacted, so Sentinel never offers to delete or shrink one.
        Total on disk: {formatBytes(w.totalDiskBytes)}.
      </p>
      {w.distros.length === 0 ? (
        <p className="text-sm">No WSL distributions are registered for this user.</p>
      ) : (
        <ul className="text-sm">
          {w.distros.map((d) => (
            <li key={d.name} className="py-1">
              <span className="flex gap-2">
                <span className="w-0 flex-1 truncate font-medium">
                  {d.name}
                  {d.isDefault ? " (default)" : ""}
                </span>
                <span className="text-xs text-slate-500 dark:text-slate-400">
                  {d.dockerDesktop ? "Docker Desktop · " : ""}WSL {d.version ?? "?"}
                </span>
                <span className="w-24 text-right tabular-nums">{d.disks.length ? formatBytes(d.diskBytes) : "—"}</span>
              </span>
              {d.disks.map((k) => (
                <span key={k.path} className="block truncate text-xs text-slate-500 dark:text-slate-400" title={k.path}>
                  {k.path} · {formatBytes(k.bytes)}
                  {k.modifiedMs !== null && ` · changed ${formatDate(k.modifiedMs)}`}
                </span>
              ))}
            </li>
          ))}
        </ul>
      )}
      {w.warnings.map((x) => (
        <p key={x} className="mt-2 text-xs text-amber-700 dark:text-amber-300">
          {x}
        </p>
      ))}
    </Panel>
  );
}

export function Containers() {
  const [state, setState] = useState<
    | { s: "idle" }
    | { s: "loading" }
    | { s: "done"; docker: DockerOverview; wsl: WslInventory }
    | { s: "error"; message: string }
  >({ s: "idle" });
  const run = async () => {
    setState({ s: "loading" });
    try {
      const [docker, wsl] = await Promise.all([ipc.dockerOverview(), ipc.wslDistros()]);
      setState({ s: "done", docker, wsl });
    } catch (err) {
      setState({ s: "error", message: describeError(err) });
    }
  };
  return (
    <>
      <PageHeader title="Containers" subtitle="Docker images, containers and volumes, and WSL distributions. Read-only." />
      <div className="mb-6 flex items-start gap-4">
        <p className="flex-1 text-sm text-slate-600 dark:text-slate-300">
          Asks Docker's command-line client a fixed set of read-only questions. It never starts Docker or changes
          anything.
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
            <Container className="size-4" aria-hidden />
          )}
          {state.s === "done" ? "Refresh" : "Inspect"}
        </button>
      </div>
      {state.s === "error" && <ErrorNote message={state.message} />}
      {state.s === "done" && (
        <div className="space-y-6">
          <DockerPanels d={state.docker} />
          <WslPanel w={state.wsl} />
        </div>
      )}
    </>
  );
}
