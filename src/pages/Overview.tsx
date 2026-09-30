import { AlertTriangle } from "lucide-react";
import { ErrorNote, Loading, PageHeader, Panel } from "../components/ui";
import { formatBytes, formatDate, formatDelta } from "../lib/format";
import { ipc } from "../lib/ipc";
import { useCommand } from "../lib/useCommand";

function StorageSummary() {
  const drives = useCommand(ipc.listDrives);
  const trends = useCommand(ipc.driveTrends);
  if (drives.status === "loading") return <Loading />;
  if (drives.status === "error") return <ErrorNote message={drives.message} />;

  const system = drives.data.find((d) => d.isSystem);
  const low = drives.data.filter((d) => d.space?.lowSpace);
  const measured = drives.data.filter((d) => d.space);
  const totalFree = measured.reduce((sum, d) => sum + (d.space?.freeBytes ?? 0), 0);
  const systemTrend = trends.status === "ok" ? trends.data.find((t) => t.root === system?.root) : undefined;
  return (
    <div className="space-y-3 text-sm">
      {system?.space ? (
        <p>
          <span className="mr-1 text-3xl font-semibold tabular-nums">{formatBytes(system.space.freeBytes)}</span>
          free on the Windows drive ({system.root.replace(/\\$/, "")}) of{" "}
          {formatBytes(system.space.totalBytes)}.
        </p>
      ) : (
        <p className="text-slate-500">The Windows drive could not be measured.</p>
      )}
      {systemTrend && (
        <p>
          Free space on this drive changed by{" "}
          <span className="font-semibold tabular-nums">{formatDelta(systemTrend.freeChangeBytes)}</span> since{" "}
          {formatDate(systemTrend.sinceMs)}.
        </p>
      )}
      <p className="text-slate-500 dark:text-slate-400">
        {formatBytes(totalFree)} free across {measured.length} measured{" "}
        {measured.length === 1 ? "drive" : "drives"}.
      </p>
      {low.map((d) => (
        <p key={d.root} className="flex items-center gap-2 text-amber-700 dark:text-amber-300">
          <AlertTriangle className="size-4 shrink-0" aria-hidden />
          {d.root.replace(/\\$/, "")} is low on space ({formatBytes(d.space?.freeBytes ?? 0)} free).
        </p>
      ))}
      <p className="text-slate-500 dark:text-slate-400">
        Use Analyze on the Storage page to see which folders and files use this space.
      </p>
    </div>
  );
}

export function Overview() {
  const locations = useCommand(ipc.protectedLocations);
  return (
    <>
      <PageHeader title="Overview" subtitle="What Sentinel knows about this machine so far." />
      <div className="grid gap-4 md:grid-cols-2">
        <Panel title="Storage">
          <StorageSummary />
        </Panel>
        <Panel title="Safety policy">
          {locations.status === "loading" && <Loading />}
          {locations.status === "error" && <ErrorNote message={locations.message} />}
          {locations.status === "ok" && (
            <p className="text-sm">
              <span className="mr-1 text-3xl font-semibold tabular-nums">{locations.data.length}</span>
              protected locations are active, in addition to built-in rules for Git data, SSH keys,
              environment files and credential stores.
            </p>
          )}
        </Panel>
      </div>
    </>
  );
}
