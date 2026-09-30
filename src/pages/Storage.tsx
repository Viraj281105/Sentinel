import { ScanSearch } from "lucide-react";
import { DriveCard } from "../components/DriveCard";
import { ScanProgressView, ScanResults } from "../components/ScanResults";
import { ErrorNote, Loading, PageHeader, Panel } from "../components/ui";
import { ipc } from "../lib/ipc";
import { useCommand } from "../lib/useCommand";
import { useScan } from "../lib/useScan";

export function Storage() {
  const drives = useCommand(ipc.listDrives);
  const trends = useCommand(ipc.driveTrends);
  const { state: scan, start, cancel } = useScan();
  const busy = !scan.loaded || scan.running !== null;

  return (
    <>
      <PageHeader
        title="Storage"
        subtitle="Drives on this computer, read directly from Windows. Figures use Explorer's units."
      />
      {drives.status === "loading" && <Loading />}
      {drives.status === "error" && <ErrorNote message={drives.message} />}
      {drives.status === "ok" &&
        (drives.data.length === 0 ? (
          <p className="text-sm text-slate-500">Windows reported no drives.</p>
        ) : (
          <div className="grid gap-4 lg:grid-cols-2">
            {drives.data.map((d) => (
              <DriveCard
                key={d.root}
                drive={d}
                trend={trends.status === "ok" ? trends.data.find((t) => t.root === d.root) : undefined}
                action={
                  d.status.state === "ready" && (
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => void start(d.root)}
                      aria-label={`Analyze ${d.root}`}
                      className="flex items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-emerald-700 disabled:cursor-not-allowed disabled:opacity-40"
                    >
                      <ScanSearch className="size-4" aria-hidden />
                      Analyze
                    </button>
                  )
                }
              />
            ))}
          </div>
        ))}

      <div className="mt-6 space-y-4">
        {scan.error && <ErrorNote message={scan.error} />}
        {scan.running && <ScanProgressView run={scan.running} onCancel={(id) => void cancel(id)} />}
        {!scan.running && scan.last && <ScanResults last={scan.last} />}
        {scan.loaded && !scan.running && !scan.last && (
          <Panel title="What is using the space">
            <p className="text-sm text-slate-500 dark:text-slate-400">
              Choose Analyze on a drive to see which folders and files use its space. Analysis only
              reads folder listings; it never opens file contents or changes anything.
            </p>
          </Panel>
        )}
      </div>
    </>
  );
}
