import { DriveCard } from "../components/DriveCard";
import { ErrorNote, Loading, PageHeader, Panel } from "../components/ui";
import { ipc } from "../lib/ipc";
import { useCommand } from "../lib/useCommand";

export function Storage() {
  const drives = useCommand(ipc.listDrives);
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
              <DriveCard key={d.root} drive={d} />
            ))}
          </div>
        ))}
      <div className="mt-6">
        <Panel title="What is using the space">
          <p className="text-sm text-slate-500 dark:text-slate-400">
            Directory scanning and category breakdown are not implemented yet (phase 2, next
            milestone).
          </p>
        </Panel>
      </div>
    </>
  );
}
