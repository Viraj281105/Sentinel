import { Lock } from "lucide-react";
import { ErrorNote, Loading, PageHeader, Panel } from "../components/ui";
import { ipc } from "../lib/ipc";
import { useCommand } from "../lib/useCommand";

export function Settings() {
  const info = useCommand(ipc.appInfo);
  const locations = useCommand(ipc.protectedLocations);
  return (
    <>
      <PageHeader title="Settings" subtitle="Safety policy and diagnostics." />
      <div className="space-y-4">
        <Panel title="Protected locations">
          <p className="mb-3 text-sm text-slate-500 dark:text-slate-400">
            Sentinel never modifies these locations or anything inside them. The list is derived
            from Windows known folders on this machine and cannot be edited.
          </p>
          {locations.status === "loading" && <Loading />}
          {locations.status === "error" && <ErrorNote message={locations.message} />}
          {locations.status === "ok" && (
            <ul className="divide-y divide-slate-100 dark:divide-slate-800">
              {locations.data.map((loc) => (
                <li key={loc.path} className="flex items-center gap-3 py-2 text-sm">
                  <Lock className="size-3.5 shrink-0 text-emerald-500" aria-hidden />
                  <span className="flex-1 truncate font-mono text-xs" title={loc.path}>
                    {loc.path}
                  </span>
                  <span className="text-slate-500 dark:text-slate-400">{loc.reason}</span>
                </li>
              ))}
            </ul>
          )}
        </Panel>
        <Panel title="Diagnostics">
          {info.status === "loading" && <Loading />}
          {info.status === "error" && <ErrorNote message={info.message} />}
          {info.status === "ok" && (
            <dl className="grid grid-cols-[8rem_1fr] gap-y-1 text-sm">
              <dt className="text-slate-500">Version</dt>
              <dd>
                {info.data.version}
                {info.data.debugBuild && " (debug build)"}
              </dd>
              <dt className="text-slate-500">Log folder</dt>
              <dd className="truncate font-mono text-xs" title={info.data.logDir}>
                {info.data.logDir}
              </dd>
            </dl>
          )}
        </Panel>
      </div>
    </>
  );
}
