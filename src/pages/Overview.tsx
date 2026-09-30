import { ErrorNote, Loading, PageHeader, Panel } from "../components/ui";
import { ipc } from "../lib/ipc";
import { useCommand } from "../lib/useCommand";

export function Overview() {
  const locations = useCommand(ipc.protectedLocations);
  return (
    <>
      <PageHeader title="Overview" subtitle="What Sentinel knows about this machine so far." />
      <div className="grid gap-4 md:grid-cols-2">
        <Panel title="Storage">
          <p className="text-sm text-slate-500 dark:text-slate-400">
            Drive discovery and storage analysis are not implemented yet (phase 2).
          </p>
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
